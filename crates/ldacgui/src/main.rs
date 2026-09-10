// ldacgui: desktop control panel for ldac-win.
//
// Mirrors the Alternative A2DP Driver workflow (device list on the left,
// driver/codec choice on the right, parameters apply on reconnect) with the
// user-mode LDAC engine underneath: the radio belongs either to Windows or
// to ldacsrc, switched with the same tools/ldacmode.ps1 script, and streams
// run as a supervised ldacsrc child whose status lines feed the health view.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// settings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DeviceEntry {
    name: String,
    addr: String,
    quality: String,
    abr: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Settings {
    devices: Vec<DeviceEntry>,
    selected: usize,
}

fn settings_path(exe_dir: &std::path::Path) -> PathBuf {
    exe_dir.join("ldacgui.json")
}

fn load_settings(path: &std::path::Path) -> Settings {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_settings(path: &std::path::Path, s: &Settings) {
    if let Ok(t) = serde_json::to_string_pretty(s) {
        let _ = std::fs::write(path, t);
    }
}

// ---------------------------------------------------------------------------
// windows paired-device list (read-only, via reg.exe)
// ---------------------------------------------------------------------------

fn fmt_addr(key: &str) -> Option<String> {
    if key.len() != 12 || !key.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let b: Vec<String> = (0..6).map(|i| key[2 * i..2 * i + 2].to_uppercase()).collect();
    Some(b.join(":"))
}

fn decode_name(kind: &str, hex: &str) -> String {
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&hex[i..i + 2.min(hex.len() - i)], 16).ok())
        .collect();
    if kind == "REG_SZ" || kind == "REG_EXPAND_SZ" {
        String::from_utf8_lossy(&bytes).trim_matches('\0').to_string()
    } else {
        let u16s: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&u16s)
            .trim_matches('\0')
            .trim()
            .to_string()
    }
}

/// (addr, name) pairs from HKLM\...\BTHPORT\Parameters\Devices.
fn win_paired_devices() -> Vec<(String, String)> {
    let out = Command::new("reg")
        .args([
            "query",
            r"HKLM\SYSTEM\CurrentControlSet\Services\BTHPORT\Parameters\Devices",
            "/s",
        ])
        .output();
    let mut devs = Vec::new();
    let Ok(out) = out else { return devs };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut cur: Option<String> = None;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("HKEY_") {
            cur = t.rsplit('\\').next().and_then(fmt_addr);
        } else if let Some(addr) = cur.clone() {
            // "    Name    REG_BINARY    5700..."
            let parts: Vec<&str> = t.split_whitespace().collect();
            if parts.len() >= 3 && parts[0] == "Name" {
                let name = decode_name(parts[1], &parts[2..].concat());
                if !name.is_empty() {
                    devs.push((addr, name));
                }
                cur = None;
            }
        }
    }
    devs
}

// ---------------------------------------------------------------------------
// radio switch + health parsing
// ---------------------------------------------------------------------------

fn repo_paths() -> (PathBuf, PathBuf) {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
    let dir = exe.parent().map(|p| p.to_path_buf()).unwrap_or(PathBuf::from("."));
    // release layout: <repo>/target/release/*.exe and <repo>/tools/ldacmode.ps1
    let script = dir
        .ancestors()
        .map(|a| a.join("tools").join("ldacmode.ps1"))
        .find(|p| p.exists())
        .unwrap_or_else(|| dir.join("ldacmode.ps1"));
    (dir, script)
}

fn radio_status(script: &std::path::Path) -> (String, String) {
    let out = Command::new("pwsh")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(script)
        .arg("status")
        .output();
    match out {
        Ok(o) => {
            let t = String::from_utf8_lossy(&o.stdout).into_owned();
            let mode = t
                .lines()
                .find_map(|l| l.strip_prefix("mode:"))
                .map(|m| m.trim().to_string())
                .unwrap_or_else(|| "unknown".into());
            (mode, t)
        }
        Err(e) => ("error".into(), e.to_string()),
    }
}

#[derive(Debug, Default, Clone)]
struct Health {
    state: String,
    buffered_ms: u64,
    rt: f64,
    fps: u64,
    bpf: u64,
    packets: u64,
    frames: u64,
    wire: u64,
    bitrate: i64,
    eqmid: i64,
    over: u64,
    under: u64,
    encerr: u64,
    senderr: u64,
    reconnects: u64,
}

fn num_after(s: &str, key: &str) -> u64 {
    s.split(key).nth(1).map(|r| num_head(r)).unwrap_or(0)
}

fn num_head(s: &str) -> u64 {
    s.split_whitespace()
        .next()
        .unwrap_or("0")
        .trim_end_matches(|c: char| !c.is_ascii_digit())
        .parse()
        .unwrap_or(0)
}

fn parse_status(line: &str) -> Option<Health> {
    // "[Streaming] buffered 20 ms | rt 1.00x | 374 frame/s, 110 B/frame | ..."
    let (state, rest) = line.strip_prefix('[')?.split_once(']')?;
    let cols: Vec<&str> = rest.split('|').collect();
    if cols.len() < 8 {
        return None;
    }
    let fps_bpf: Vec<&str> = cols[2].split(',').collect();
    Some(Health {
        state: state.to_string(),
        buffered_ms: num_after(cols[1], "buffered"),
        rt: cols[2 - 1]
            .split("rt")
            .nth(1)
            .and_then(|r| r.trim().trim_end_matches('x').parse().ok())
            .unwrap_or(0.0),
        fps: num_head(fps_bpf.first().unwrap_or(&"0")),
        bpf: fps_bpf.get(1).map(|s| num_after(s, "")).unwrap_or(0),
        packets: num_after(cols[3], "packets"),
        frames: num_after(cols[3].split("packets").nth(1).unwrap_or(""), "frames")
            .max(num_after(cols[3], "frames")),
        wire: num_after(cols[4], "wire"),
        bitrate: num_after(cols[5], "ldac") as i64,
        eqmid: cols[5].split("eqmid").nth(1).map(num_head).unwrap_or(0) as i64,
        over: num_after(cols[6], "over"),
        under: num_after(cols[6].split("over").nth(1).unwrap_or(""), "under")
            .max(num_after(cols[6], "under")),
        encerr: num_after(cols[7], "encerr"),
        senderr: num_after(cols[7].split("encerr").nth(1).unwrap_or(""), "senderr")
            .max(num_after(cols[7], "senderr")),
        reconnects: cols.get(8).map(|s| num_after(s, "reconnects")).unwrap_or(0),
    })
}

enum GuiMsg {
    Line(String),
    Exited,
    RadioDone(String, String),
}

// ---------------------------------------------------------------------------
// app
// ---------------------------------------------------------------------------

struct App {
    settings: Settings,
    exe_dir: PathBuf,
    script: PathBuf,
    radio_mode: String,
    radio_raw: String,
    radio_busy: bool,
    running: bool,
    child: Option<Child>,
    rx: Option<Receiver<GuiMsg>>,
    tx: Option<Sender<GuiMsg>>,
    events: Vec<String>,
    health: Option<Health>,
    last_over: u64,
    hint: String,
    new_name: String,
    new_addr: String,
    win_devs: Vec<(String, String)>,
}

impl App {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let (exe_dir, script) = repo_paths();
        let sp = settings_path(&exe_dir);
        let mut settings = load_settings(&sp);
        if settings.devices.is_empty() {
            settings.devices.push(DeviceEntry {
                name: "WH-1000XM3".into(),
                addr: "14:3F:A6:35:D0:AA".into(),
                quality: "sq".into(),
                abr: false,
            });
        }
        if settings.selected >= settings.devices.len() {
            settings.selected = 0;
        }
        let (mode, raw) = radio_status(&script);
        let win_devs = win_paired_devices();
        Self {
            settings,
            exe_dir,
            script,
            radio_mode: mode,
            radio_raw: raw,
            radio_busy: false,
            running: false,
            child: None,
            rx: None,
            tx: None,
            events: vec!["ready".into()],
            health: None,
            last_over: 0,
            hint: String::new(),
            new_name: String::new(),
            new_addr: String::new(),
            win_devs,
        }
    }

    fn persist(&self) {
        save_settings(&settings_path(&self.exe_dir), &self.settings);
    }

    fn log(&mut self, s: String) {
        self.events.push(s);
        if self.events.len() > 300 {
            let n = self.events.len() - 300;
            self.events.drain(..n);
        }
    }

    fn pump(&mut self) {
        let mut radio_refresh = false;
        let msgs: Vec<GuiMsg> = if let Some(rx) = &self.rx {
            rx.try_iter().collect()
        } else {
            Vec::new()
        };
        for m in msgs {
            match m {
                    GuiMsg::Line(l) => {
                        if let Some(h) = parse_status(&l) {
                            if h.over > self.last_over && self.last_over > 0 {
                                self.hint = "overruns rising: link can't hold this bitrate — drop to mq (stutter fix)".into();
                            }
                            self.last_over = h.over;
                            self.health = Some(h);
                        } else if !l.trim().is_empty() {
                            self.log(l);
                        }
                    }
                    GuiMsg::Exited => {
                        self.running = false;
                        self.child = None;
                        self.log("ldacsrc exited".into());
                    }
                GuiMsg::RadioDone(mode, raw) => {
                    self.radio_mode = mode;
                    self.radio_raw = raw;
                    self.radio_busy = false;
                    radio_refresh = true;
                }
            }
        }
        if radio_refresh {
            self.win_devs = win_paired_devices();
        }
    }

    fn switch_radio(&mut self, target: &str) {
        if self.running || self.radio_busy {
            return;
        }
        self.radio_busy = true;
        self.log(format!("switching radio to {target} (elevates itself)…"));
        let script = self.script.clone();
        let target = target.to_string();
        let tx = self.tx_clone();
        std::thread::spawn(move || {
            let out = Command::new("pwsh")
                .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
                .arg(&script)
                .arg(&target)
                .output();
            let (mode, raw) = match out {
                Ok(o) => {
                    let t = String::from_utf8_lossy(&o.stdout).into_owned()
                        + &String::from_utf8_lossy(&o.stderr);
                    let mode = t
                        .lines()
                        .find_map(|l| l.strip_prefix("mode:"))
                        .map(|m| m.trim().to_string())
                        .unwrap_or_else(|| format!("exit {}", o.status));
                    (mode, t)
                }
                Err(e) => ("error".into(), e.to_string()),
            };
            let _ = tx.send(GuiMsg::RadioDone(mode, raw));
        });
    }

    fn tx_clone(&mut self) -> Sender<GuiMsg> {
        if self.tx.is_none() {
            let (tx, rx) = mpsc::channel();
            self.tx = Some(tx);
            self.rx = Some(rx);
        }
        self.tx.clone().unwrap()
    }

    fn start_stream(&mut self) {
        if self.running {
            return;
        }
        let Some(dev) = self.settings.devices.get(self.settings.selected).cloned() else { return };
        let bin = self.exe_dir.join("ldacsrc.exe");
        if !bin.exists() {
            self.log(format!("build missing: {}", bin.display()));
            return;
        }
        self.persist();
        let mut cmd = Command::new(&bin);
        cmd.args(["--addr", &dev.addr, "--quality", &dev.quality]);
        if dev.abr {
            cmd.arg("--abr");
        }
        cmd.current_dir(
            bin.parent()
                .and_then(|p| p.parent())
                .and_then(|p| p.parent())
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| self.exe_dir.clone()),
        );
        cmd.stdout(Stdio::piped()).stderr(Stdio::null());
        match cmd.spawn() {
            Ok(mut child) => {
                let tx = self.tx_clone();
                if let Some(stdout) = child.stdout.take() {
                    let tx2 = tx.clone();
                    std::thread::spawn(move || {
                        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                            if tx2.send(GuiMsg::Line(line)).is_err() {
                                break;
                            }
                        }
                        let _ = tx2.send(GuiMsg::Exited);
                    });
                }
                self.child = Some(child);
                self.running = true;
                self.health = None;
                self.last_over = 0;
                self.hint.clear();
                self.log(format!("streaming to {} ({})", dev.name, dev.addr));
            }
            Err(e) => self.log(format!("spawn failed: {e}")),
        }
    }

    fn stop_stream(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
        }
        self.running = false;
        self.log("stopped".into());
    }
}

impl eframe::App for App {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.pump();

        egui::Panel::left("devices").resizable(true).show(root, |ui| {
            ui.heading("Devices");
            ui.separator();
            let mut sel = self.settings.selected;
            for (i, d) in self.settings.devices.iter().enumerate() {
                if ui.selectable_label(sel == i, format!("{}  {}", d.name, d.addr)).clicked() {
                    sel = i;
                }
            }
            if sel != self.settings.selected {
                self.settings.selected = sel;
                self.persist();
            }
            ui.separator();
            ui.label("Add device:");
            ui.text_edit_singleline(&mut self.new_name);
            ui.text_edit_singleline(&mut self.new_addr);
            ui.horizontal(|ui| {
                if ui.button("Add").clicked() && !self.new_addr.is_empty() {
                    let name = if self.new_name.is_empty() { self.new_addr.clone() } else { self.new_name.clone() };
                    self.settings.devices.push(DeviceEntry {
                        name,
                        addr: self.new_addr.trim().to_uppercase(),
                        quality: "sq".into(),
                        abr: false,
                    });
                    self.new_name.clear();
                    self.new_addr.clear();
                    self.persist();
                }
                if ui.button("Remove").clicked() && self.settings.devices.len() > 1 {
                    self.settings.devices.remove(self.settings.selected);
                    self.settings.selected = 0;
                    self.persist();
                }
            });
            ui.separator();
            ui.label("Windows-paired (read-only):");
            egui::ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
                let mut known: BTreeMap<String, String> = BTreeMap::new();
                for d in &self.settings.devices {
                    known.insert(d.addr.clone(), d.name.clone());
                }
                for (addr, name) in &self.win_devs {
                    let mark = if known.contains_key(addr) { "[in list]" } else { "[new]" };
                    if ui.selectable_label(false, format!("{mark} {name}  {addr}")).clicked() {
                        self.new_name = name.clone();
                        self.new_addr = addr.clone();
                    }
                }
                if self.win_devs.is_empty() {
                    ui.weak("none visible (radio may be in LDAC mode)");
                }
            });
            if ui.button("Refresh paired").clicked() {
                self.win_devs = win_paired_devices();
            }
        });

        egui::CentralPanel::default().show(root, |ui| {
            ui.heading("LDACast");
            ui.separator();

            // radio row (Goodies "device driver" switch equivalent)
            ui.horizontal(|ui| {
                ui.label("Radio:");
                let col = match self.radio_mode.as_str() {
                    "bt" => egui::Color32::LIGHT_BLUE,
                    "ldac" => egui::Color32::YELLOW,
                    _ => egui::Color32::GRAY,
                };
                ui.colored_label(col, &self.radio_mode);
                if self.radio_busy {
                    ui.spinner();
                } else {
                    let can = !self.running;
                    ui.add_enabled_ui(can, |ui| {
                        if ui.button("To Windows").clicked() {
                            self.switch_radio("bt");
                        }
                        if ui.button("To LDAC").clicked() {
                            self.switch_radio("ldac");
                        }
                    });
                }
                if ui.button("Status").clicked() && !self.radio_busy {
                    let (m, r) = radio_status(&self.script);
                    self.radio_mode = m;
                    self.radio_raw = r;
                }
            });
            if self.radio_mode != "ldac" && !self.radio_busy {
                ui.colored_label(egui::Color32::YELLOW, "radio is not in LDAC mode — streaming needs it");
            }

            ui.separator();

            // codec params (Goodies CODEC tuning equivalent)
            {
                let sel = self.settings.selected;
                if let Some(dev) = self.settings.devices.get_mut(sel) {
                    ui.horizontal(|ui| {
                        ui.label("CODEC:");
                        ui.label("LDAC");
                        ui.separator();
                        ui.label("Quality:");
                        egui::ComboBox::from_id_salt("q").selected_text(&dev.quality).show_ui(ui, |ui| {
                            for q in ["hq", "sq", "mq"] {
                                ui.selectable_value(&mut dev.quality, q.to_string(), match q {
                                    "hq" => "hq — 909/990 kbps",
                                    "sq" => "sq — 606/660 kbps",
                                    "mq" => "mq — 303/330 kbps",
                                    _ => q,
                                });
                            }
                        });
                        ui.checkbox(&mut dev.abr, "ABR");
                    });
                }
            }
            ui.weak("stuttering? drop a rung (hq > sq > mq): lower bitrate survives poor radio. applies on (re)connect.");
            if !self.hint.is_empty() {
                ui.colored_label(egui::Color32::LIGHT_RED, &self.hint);
            }

            ui.separator();

            // stream control
            ui.horizontal(|ui| {
                let can_start = !self.running && self.radio_mode == "ldac";
                if ui.add_enabled(can_start, egui::Button::new("Start")).clicked() {
                    self.start_stream();
                }
                if ui.add_enabled(self.running, egui::Button::new("Stop")).clicked() {
                    self.stop_stream();
                }
                if self.running {
                    ui.spinner();
                    ui.label("streaming");
                }
            });

            // health (Goodies has no live view — this is the addition)
            if let Some(h) = &self.health {
                ui.separator();
                egui::Grid::new("health").num_columns(4).show(ui, |ui| {
                    ui.label("state"); ui.strong(&h.state); ui.label("wire"); ui.strong(format!("{} kbps", h.wire)); ui.end_row();
                    ui.label("realtime"); ui.strong(format!("{:.2}x", h.rt)); ui.label("buffer"); ui.strong(format!("{} ms", h.buffered_ms)); ui.end_row();
                    ui.label("codec"); ui.strong(format!("LDAC {} kbps eqmid {}", h.bitrate, h.eqmid)); ui.label("packets"); ui.strong(format!("{}", h.packets)); ui.end_row();
                    ui.label("overruns"); ui.strong(format!("{}", h.over)); ui.label("underruns"); ui.strong(format!("{}", h.under)); ui.end_row();
                });
            }

            // events
            ui.separator();
            ui.label("Events:");
            egui::ScrollArea::vertical().max_height(180.0).stick_to_bottom(true).show(ui, |ui| {
                for e in &self.events {
                    ui.monospace(e);
                }
            });
            if ui.button("Copy radio detail").clicked() {
                ui.output_mut(|o| o.commands.push(egui::OutputCommand::CopyText(self.radio_raw.clone())));
            }
        });

        if self.running || self.radio_busy {
            root.ctx().request_repaint_after(Duration::from_millis(500));
        }
    }
}

fn main() -> eframe::Result<()> {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([860.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "LDACast",
        opts,
        Box::new(|cc| Ok(Box::new(App::new(cc)) as Box<dyn eframe::App>)),
    )
}
