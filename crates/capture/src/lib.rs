pub mod ring;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use windows::core::{Result, HSTRING};
use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDevice, IMMDeviceEnumerator,
    MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK, WAVEFORMATEX,
    WAVEFORMATEXTENSIBLE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

pub const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
pub const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format {
    pub sample_rate: u32,
    pub channels: u16,
}

pub struct Capture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub format: Format,
}

impl Capture {
    /// Starts WASAPI loopback capture of the default render endpoint. Audio is
    /// delivered into `sink` from a dedicated thread; no protocol code runs on
    /// that thread.
    pub fn start(sink: ring::Producer) -> Result<Capture> {
        Self::start_inner(sink, None)
    }

    /// Starts loopback capture of the first active render endpoint whose
    /// friendly name contains `query` (case-insensitive), e.g. a virtual
    /// cable used as a dedicated silent sink. Falls back to an error naming
    /// the available endpoints when nothing matches.
    pub fn start_on(sink: ring::Producer, query: &str) -> Result<Capture> {
        let id = find_endpoint(query).ok_or_else(|| {
            let mut names: Vec<String> = list_render_endpoints().into_iter().map(|(_, n)| n).collect();
            if names.is_empty() {
                names.push("(no render endpoints found)".into());
            }
            windows::core::Error::new(
                windows::Win32::Foundation::E_INVALIDARG,
                format!("no render endpoint matches {query:?}; available: {}", names.join(", ")),
            )
        })?;
        Self::start_inner(sink, Some(id))
    }

    fn start_inner(sink: ring::Producer, endpoint_id: Option<String>) -> Result<Capture> {
        let (tx, rx) = std::sync::mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();

        let thread = std::thread::Builder::new()
            .name("wasapi-loopback".into())
            .spawn(move || {
                let result = capture_thread(sink, stop_thread, &tx, endpoint_id);
                if let Err(e) = result {
                    let _ = tx.send(Err(e));
                }
            })
            .expect("spawn capture thread");

        match rx.recv() {
            Ok(Ok(format)) => Ok(Capture { stop, thread: Some(thread), format }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(windows::core::Error::from_win32()),
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn describe_format(wf: *const WAVEFORMATEX) -> Result<Format> {
    let base = unsafe { *wf };
    let (bits, channels, rate) = (base.wBitsPerSample, base.nChannels, base.nSamplesPerSec);
    let tag = if base.wFormatTag == WAVE_FORMAT_EXTENSIBLE {
        let ext = wf as *const WAVEFORMATEXTENSIBLE;
        let sub = unsafe { (*ext).SubFormat };
        // KSDATAFORMAT_SUBTYPE_IEEE_FLOAT has the format tag in Data1
        sub.data1 as u16
    } else {
        base.wFormatTag
    };
    if tag != WAVE_FORMAT_IEEE_FLOAT || bits != 32 {
        return Err(windows::core::Error::new(
            windows::Win32::Foundation::E_NOTIMPL,
            format!("unsupported mix format: tag {tag:#x}, {bits} bit, {channels} ch, {rate} Hz"),
        ));
    }
    Ok(Format { sample_rate: rate, channels })
}

/// Active render endpoints as (id, friendly name), read from the MMDevices
/// registry key. Friendly names match what Sound settings shows.
pub fn list_render_endpoints() -> Vec<(String, String)> {
    const BASE: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Render";
    const NAME_PROP: &str = "{a45c254e-df1c-4efd-8020-67d146a850e0},2";
    let mut out = Vec::new();
    let Ok(hklm) = winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE).open_subkey(BASE) else {
        return out;
    };
    for id in hklm.enum_keys().filter_map(|r| r.ok()) {
        let Ok(dev) = hklm.open_subkey(&id) else { continue };
        let state: u32 = dev.get_value("DeviceState").unwrap_or(0);
        if state != 1 {
            continue;
        }
        let name: String = dev
            .open_subkey("Properties")
            .ok()
            .and_then(|p| p.get_value(NAME_PROP).ok())
            .unwrap_or_else(|| id.clone());
        out.push((id, name));
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

/// First active endpoint id whose friendly name contains `query` (case-insensitive).
pub fn find_endpoint(query: &str) -> Option<String> {
    let q = query.to_lowercase();
    list_render_endpoints()
        .into_iter()
        .find(|(_, name)| name.to_lowercase().contains(&q))
        .map(|(id, _)| id)
}

fn capture_thread(
    sink: ring::Producer,
    stop: Arc<AtomicBool>,
    ready: &std::sync::mpsc::Sender<Result<Format>>,
    endpoint_id: Option<String>,
) -> Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        let guard = ComGuard;

        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device: IMMDevice = match endpoint_id {
            Some(id) => {
                // Registry key names carry the bare GUID; endpoint IDs take
                // the "{0.0.0.00000000}." device-interface prefix.
                enumerator
                    .GetDevice(&HSTRING::from(id.clone()))
                    .or_else(|_| {
                        enumerator.GetDevice(&HSTRING::from(format!("{{0.0.0.00000000}}.{id}")))
                    })?
            }
            None => enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?,
        };
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;

        let wf = client.GetMixFormat()?;
        let format = match describe_format(wf) {
            Ok(f) => f,
            Err(e) => {
                CoTaskMemFree(Some(wf as *const _));
                let _ = ready.send(Err(e.clone()));
                return Err(e);
            }
        };

        // 200 ms endpoint buffer; loopback ignores the period argument
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            2_000_000,
            0,
            wf,
            None,
        )?;
        CoTaskMemFree(Some(wf as *const _));

        let event: HANDLE = CreateEventW(None, false, false, None)?;
        client.SetEventHandle(event)?;
        let capture: IAudioCaptureClient = client.GetService()?;
        client.Start()?;

        let _ = ready.send(Ok(format));

        while !stop.load(Ordering::Relaxed) {
            if WaitForSingleObject(event, 200) != WAIT_OBJECT_0 {
                continue;
            }
            loop {
                let mut frames = 0u32;
                let mut data = std::ptr::null_mut();
                let mut flags = 0u32;
                if capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                    .is_err()
                {
                    break;
                }
                if frames == 0 {
                    let _ = capture.ReleaseBuffer(0);
                    break;
                }
                let count = frames as usize * format.channels as usize;
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                    sink.push(&vec![0.0; count]);
                } else {
                    sink.push(std::slice::from_raw_parts(data as *const f32, count));
                }
                capture.ReleaseBuffer(frames)?;
            }
        }

        client.Stop()?;
        drop(guard);
    }
    Ok(())
}

struct ComGuard;

impl Drop for ComGuard {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}
