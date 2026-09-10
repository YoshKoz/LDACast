mod codec;
mod encoder;
mod packer;

use std::ffi::CString;

use btstack_sys as bt;
use capture::ring;
use codec::LdacCaps;
use encoder::Encoder;
use packer::{Packer, Push};

const AUDIO_TIMER_MS: u32 = 10;
const STATS_TIMER_MS: u32 = 5_000;
const RECONNECT_DELAY_MS: u32 = 3_000;
/// Audio buffered before the first packet goes out.
const PRIME_MS: u32 = 40;
const RING_SECONDS: usize = 2;
/// Class of Device: rendering / audio, audio-video major class.
const COD_AUDIO_SOURCE: u32 = 0x0020_0408;
const SPEAKER_COD_MASK: u32 = 0x0020_0000 | 0x0000_0400;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    Scanning,
    Connecting,
    Bonding,
    Configuring,
    Streaming,
    Reconnecting,
}

struct Stats {
    packets: u64,
    frames: u64,
    payload_bytes: u64,
    samples_consumed: u64,
    encode_calls: u64,
    encode_errors: u64,
    send_errors: u64,
    reconnects: u64,
    last_report_ms: u32,
}

struct App {
    target: Option<[u8; 6]>,
    state: State,
    a2dp_cid: u16,
    local_seid: u8,
    ldac_remote_seid: Option<u8>,
    sink_caps: Option<LdacCaps>,
    negotiated: Option<LdacCaps>,
    enc: Option<Encoder>,
    packer: Option<Packer>,
    /// Payloads waiting for a can-send slot, with the samples each represents.
    ready: std::collections::VecDeque<(Vec<u8>, u32)>,
    pcm: Vec<f32>,
    audio: ring::Consumer,
    format: capture::Format,
    eqmid: i32,
    abr: bool,
    /// Auto Windows fallback: after a stream was established at least once
    /// and then lost for this many seconds, hand the radio back to Windows
    /// and exit. 0 disables. Set from --auto-bt-fallback.
    fallback_after_s: u32,
    streamed_once: bool,
    stream_lost_ms: u32,
    rtp_timestamp: u32,
    pending_samples: u32,
    awaiting_can_send: bool,
    primed: bool,
    audio_timer: bt::btstack_timer_source_t,
    stats_timer: bt::btstack_timer_source_t,
    reconnect_timer: bt::btstack_timer_source_t,
    audio_delay_timer: bt::btstack_timer_source_t,
    /// Short delay between bonding-complete (which disconnects the ACL)
    /// and the audio connect: connecting immediately collides with the
    /// still-tearing-down connection object and the controller never gets
    /// a Create_Connection at all.
    stats: Stats,
    // BTstack keeps these pointers and reads them when it builds AVDTP
    // messages, so they must outlive every call that hands them over
    local_caps: [u8; 8],
    local_config: [u8; 8],
    set_config_info: [u8; 8],
    sdp_a2dp: [u8; 150],
    hci_cb: bt::btstack_packet_callback_registration_t,
    tlv_ctx: bt::btstack_tlv_windows_t,
    tlv_path: Option<CString>,
}

static mut APP: Option<Box<App>> = None;

fn app() -> &'static mut App {
    unsafe {
        let ptr = std::ptr::addr_of_mut!(APP);
        (*ptr).as_mut().expect("app not initialised")
    }
}

fn addr_str(addr: &[u8; 6]) -> String {
    addr.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

fn parse_addr(s: &str) -> Option<[u8; 6]> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 6 {
        return None;
    }
    let mut out = [0u8; 6];
    for (i, p) in parts.iter().enumerate() {
        out[i] = u8::from_str_radix(p, 16).ok()?;
    }
    Some(out)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut target = None;
    let mut eqmid = encoder::EQMID_SQ;
    let mut abr = false;
    let mut check_capture = false;
    let mut capture_device: Option<String> = None;
    let mut fallback_after_s: u32 = 0;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--addr" => {
                let v = iter.next().expect("--addr needs BD_ADDR");
                target = Some(parse_addr(v).expect("bad BD_ADDR, expected AA:BB:CC:DD:EE:FF"));
            }
            "--abr" => abr = true,
            "--auto-bt-fallback" => {
                fallback_after_s = iter
                    .next()
                    .expect("--auto-bt-fallback needs seconds")
                    .parse()
                    .expect("--auto-bt-fallback needs a number of seconds");
            }
            "--capture" => {
                capture_device = Some(iter.next().expect("--capture needs a device name fragment").clone());
            }
            "--check-capture" => check_capture = true,
            "--list-capture" => {
                for (id, name) in capture::list_render_endpoints() {
                    println!("{name}  [{id}]");
                }
                return;
            }
            "--quality" => {
                eqmid = match iter.next().map(|s| s.as_str()) {
                    Some("hq") => encoder::EQMID_HQ,
                    Some("sq") => encoder::EQMID_SQ,
                    Some("mq") => encoder::EQMID_MQ,
                    other => panic!("--quality expects hq|sq|mq, got {other:?}"),
                };
            }
            "--help" | "-h" => {
                println!("ldacsrc [--addr AA:BB:CC:DD:EE:FF] [--quality hq|sq|mq] [--abr] [--capture NAME] [--auto-bt-fallback SECS] [--check-capture] [--list-capture]");
                println!("without --addr the first audio sink found by inquiry is used");
                println!("without --capture the default playback device is captured; --list-capture shows names");
                println!("--auto-bt-fallback SECS hands the radio back to Windows (via ldacmode.ps1, UAC) once");
                println!("a stream was established and then lost for SECS seconds; 0 (default) disables");
                return;
            }
            other => panic!("unknown argument {other}"),
        }
    }

    let (producer, consumer) = ring::ring(48_000 * 2 * RING_SECONDS);
    let cap = match capture_device {
        Some(ref q) => capture::Capture::start_on(producer, q),
        None => capture::Capture::start(producer),
    };
    let cap = match cap {
        Ok(c) => c,
        Err(e) => {
            eprintln!("WASAPI loopback capture failed: {e}");
            std::process::exit(1);
        }
    };
    let format = cap.format;
    println!("capture: {} Hz, {} ch, f32 loopback", format.sample_rate, format.channels);
    if codec::freq_bit(format.sample_rate).is_none() {
        eprintln!(
            "the Windows mix format is {} Hz, which LDAC cannot carry.",
            format.sample_rate
        );
        eprintln!("set the shared-mode format of the default playback device to 44.1/48/88.2/96 kHz.");
        std::process::exit(2);
    }

    if !matches!(format.channels, 1 | 2) {
        eprintln!("LDAC requires mono or stereo capture, got {} channels", format.channels);
        std::process::exit(2);
    }
    if check_capture {
        return;
    }

    unsafe {
        APP = Some(Box::new(App {
            target,
            state: State::Idle,
            a2dp_cid: 0,
            local_seid: 0,
            ldac_remote_seid: None,
            sink_caps: None,
            negotiated: None,
            enc: None,
            packer: None,
            ready: std::collections::VecDeque::new(),
            pcm: vec![0.0; encoder::FRAME_SAMPLES * format.channels as usize],
            audio: consumer,
            format,
            eqmid,
            abr,
            fallback_after_s,
            streamed_once: false,
            stream_lost_ms: 0,
            rtp_timestamp: 0,
            pending_samples: 0,
            awaiting_can_send: false,
            primed: false,
            audio_timer: std::mem::zeroed(),
            stats_timer: std::mem::zeroed(),
            reconnect_timer: std::mem::zeroed(),
            audio_delay_timer: std::mem::zeroed(),
            stats: Stats {
                packets: 0,
                frames: 0,
                payload_bytes: 0,
                samples_consumed: 0,
                encode_calls: 0,
                encode_errors: 0,
                send_errors: 0,
                reconnects: 0,
                last_report_ms: 0,
            },
            local_caps: [0; 8],
            local_config: [0; 8],
            set_config_info: [0; 8],
            sdp_a2dp: [0; 150],
            hci_cb: std::mem::zeroed(),
            tlv_ctx: std::mem::zeroed(),
            tlv_path: None,
        }));

        setup_btstack();
        bt::hci_power_control(bt::HCI_POWER_MODE_HCI_POWER_ON);
        bt::btstack_run_loop_execute();
    }

    drop(cap);
}

unsafe fn setup_btstack() { unsafe {
    bt::btstack_memory_init();
    bt::btstack_run_loop_init(bt::btstack_run_loop_windows_get_instance());

    let pklg = CString::new("ldacsrc.pklg").unwrap();
    bt::hci_dump_windows_fs_open(pklg.as_ptr(), bt::hci_dump_format_t_HCI_DUMP_PACKETLOGGER);
    bt::hci_dump_init(bt::hci_dump_windows_fs_get_instance());
    println!("packet log: ldacsrc.pklg");

    bt::hci_init(bt::hci_transport_usb_instance(), std::ptr::null());
    bt::l2cap_init();
    bt::sdp_init();

    bt::a2dp_source_init();
    bt::a2dp_source_register_packet_handler(Some(a2dp_handler));

    // Offer every rate/channel mode the local capture path can actually serve;
    // the sink picks from this in SET_CONFIGURATION.
    let offered = LdacCaps {
        sampling_freqs: codec::freq_bit(app().format.sample_rate).unwrap(),
        channel_modes: if app().format.channels == 1 { codec::CHAN_MONO } else { codec::CHAN_STEREO },
    };
    let a = app();
    a.local_caps = codec::media_codec_info(offered);
    a.local_config = a.local_caps;
    let endpoint = bt::a2dp_source_create_stream_endpoint(
        bt::avdtp_media_type_t_AVDTP_AUDIO,
        bt::avdtp_media_codec_type_t_AVDTP_CODEC_NON_A2DP,
        a.local_caps.as_ptr(),
        a.local_caps.len() as u16,
        a.local_config.as_mut_ptr(),
        a.local_config.len() as u16,
    );
    assert!(!endpoint.is_null(), "could not create LDAC stream endpoint");
    app().local_seid = bt::avdtp_local_seid(endpoint);
    println!(
        "local LDAC endpoint seid {} offering {:#04x}/{:#04x}",
        app().local_seid,
        offered.sampling_freqs,
        offered.channel_modes
    );

    let a = app();
    bt::a2dp_source_create_sdp_record(
        a.sdp_a2dp.as_mut_ptr(),
        bt::sdp_create_service_record_handle(),
        bt::AVDTP_SOURCE_FEATURE_MASK_PLAYER as u16,
        std::ptr::null(),
        std::ptr::null(),
    );
    bt::sdp_register_service(a.sdp_a2dp.as_ptr());

    bt::gap_set_local_name(c"LDAC Source 00:00:00:00:00:00".as_ptr());
    bt::gap_discoverable_control(1);
    bt::gap_set_class_of_device(COD_AUDIO_SOURCE);
    // No display, no keyboard: SSP degrades to Just Works, no user step.
    bt::gap_ssp_set_io_capability(bt::SSP_IO_CAPABILITY_NO_INPUT_NO_OUTPUT as std::ffi::c_int);

    a.hci_cb.callback = Some(hci_handler);
    bt::hci_add_event_handler(&mut a.hci_cb);
}}

unsafe extern "C" fn hci_handler(packet_type: u8, _channel: u16, packet: *mut u8, _size: u16) { unsafe {
    if packet_type != bt::HCI_EVENT_PACKET as u8 {
        return;
    }
    let a = app();
    match bt::hci_event_packet_get_type(packet) as u32 {
        bt::BTSTACK_EVENT_STATE => {
            if bt::btstack_event_state_get_state(packet) as i32 != bt::HCI_STATE_HCI_STATE_WORKING {
                return;
            }
            let mut local = [0u8; 6];
            bt::gap_local_bd_addr(local.as_mut_ptr());
            println!("radio up on {}", addr_str(&local));

            // same naming as BTstack's own ports, so link keys paired with any
            // BTstack example on this radio are reused
            let file = format!(
                "btstack_{}.tlv",
                local.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join("-")
            );
            a.tlv_path = Some(CString::new(file).unwrap());
            let tlv = bt::btstack_tlv_windows_init_instance(&mut a.tlv_ctx, a.tlv_path.as_ref().unwrap().as_ptr());
            bt::btstack_tlv_set_instance(tlv, (&mut a.tlv_ctx) as *mut _ as *mut std::ffi::c_void);
            bt::hci_set_link_key_db(bt::btstack_link_key_db_tlv_get_instance(
                tlv,
                (&mut a.tlv_ctx) as *mut _ as *mut std::ffi::c_void,
            ));

            bt::btstack_run_loop_set_timer_handler(&mut a.stats_timer, Some(stats_timer_handler));
            bt::btstack_run_loop_set_timer(&mut a.stats_timer, STATS_TIMER_MS);
            bt::btstack_run_loop_add_timer(&mut a.stats_timer);

            connect_or_scan();
        }
        bt::HCI_EVENT_PIN_CODE_REQUEST => {
            let mut addr = [0u8; 6];
            bt::hci_event_pin_code_request_get_bd_addr(packet, addr.as_mut_ptr());
            println!("legacy pairing requested by {} - refused", addr_str(&addr));
            bt::gap_pin_code_negative(addr.as_mut_ptr());
        }
        bt::HCI_EVENT_USER_CONFIRMATION_REQUEST => {
            let mut addr = [0u8; 6];
            bt::hci_event_user_confirmation_request_get_bd_addr(packet, addr.as_mut_ptr());
            println!("accepting SSP pairing with {}", addr_str(&addr));
            bt::gap_ssp_confirmation_response(addr.as_mut_ptr());
        }
        bt::GAP_EVENT_INQUIRY_RESULT => {
            if a.state != State::Scanning {
                return;
            }
            let mut addr = [0u8; 6];
            bt::gap_event_inquiry_result_get_bd_addr(packet, addr.as_mut_ptr());
            let cod = bt::gap_event_inquiry_result_get_class_of_device(packet);
            println!("found {} cod {cod:06x}", addr_str(&addr));
            if cod & SPEAKER_COD_MASK == SPEAKER_COD_MASK {
                a.target = Some(addr);
                bt::gap_inquiry_stop();
                establish();
            }
        }
        bt::GAP_EVENT_INQUIRY_COMPLETE => {
            if a.state == State::Scanning {
                println!("no audio sink found, scanning again");
                bt::gap_inquiry_start(5);
            }
        }
        bt::GAP_EVENT_DEDICATED_BONDING_COMPLETED => {
            let status = bt::gap_event_dedicated_bonding_completed_get_status(packet);
            let mut addr = [0u8; 6];
            bt::gap_event_dedicated_bonding_completed_get_address(packet, addr.as_mut_ptr());
            if status as u32 != bt::ERROR_CODE_SUCCESS {
                // Sink not pairable right now (not in pairing mode, or it
                // rejected). Try audio anyway: with a stored key the bond
                // above is what matters, without one this fails fast and
                // the reconnect loop retries once the sink is pairable.
                println!("bonding with {} exited {status:#04x}, trying audio", addr_str(&addr));
            } else {
                println!("bonded with {}, connecting audio", addr_str(&addr));
            }
            // Let the bonding teardown finish before the audio connect.
            const AUDIO_DELAY_MS: u32 = 1500;
            bt::btstack_run_loop_remove_timer(&mut a.audio_delay_timer);
            bt::btstack_run_loop_set_timer_handler(&mut a.audio_delay_timer, Some(audio_delay_timer_handler));
            bt::btstack_run_loop_set_timer(&mut a.audio_delay_timer, AUDIO_DELAY_MS);
            bt::btstack_run_loop_add_timer(&mut a.audio_delay_timer);
        }
        _ => {}
    }
}}

unsafe fn connect_or_scan() { unsafe {
    let a = app();
    if a.target.is_some() {
        establish();
    } else {
        println!("scanning for an audio sink");
        a.state = State::Scanning;
        bt::gap_inquiry_start(5);
    }
}}

unsafe fn establish() { unsafe {
    // Bond first, audio second. With a stored link key this is a silent
    // re-authentication; without one it pairs (Just Works, see the IO cap
    // in setup). The AVDTP connect that follows then finds security
    // level 2 already satisfied instead of failing with 0x66.
    let a = app();
    if a.target.is_none() {
        return;
    }
    let mut addr = a.target.expect("no target address");
    a.state = State::Bonding;
    println!("bonding with {}", addr_str(&addr));
    bt::gap_dedicated_bonding(addr.as_mut_ptr(), 0);
}}

unsafe fn establish_audio() { unsafe {
    let a = app();
    let mut addr = a.target.expect("no target address");
    a.state = State::Connecting;
    println!("connecting to {}", addr_str(&addr));
    let status = bt::a2dp_source_establish_stream(addr.as_mut_ptr(), &mut a.a2dp_cid);
    if status as u32 != bt::ERROR_CODE_SUCCESS {
        println!("connect failed with status {status:#04x}, retrying");
        schedule_reconnect();
    }
}}

unsafe fn schedule_reconnect() { unsafe {
    let a = app();
    if a.target.is_none() {
        a.state = State::Idle;
        return;
    }
    stop_streaming();
    a.state = State::Reconnecting;
    a.stats.reconnects += 1;
    bt::btstack_run_loop_remove_timer(&mut a.reconnect_timer);
    bt::btstack_run_loop_set_timer_handler(&mut a.reconnect_timer, Some(reconnect_timer_handler));
    bt::btstack_run_loop_set_timer(&mut a.reconnect_timer, RECONNECT_DELAY_MS);
    bt::btstack_run_loop_add_timer(&mut a.reconnect_timer);
}}

unsafe extern "C" fn reconnect_timer_handler(_t: *mut bt::btstack_timer_source_t) { unsafe {
    establish();
}}

unsafe extern "C" fn audio_delay_timer_handler(_t: *mut bt::btstack_timer_source_t) { unsafe {
    establish_audio();
}}

unsafe extern "C" fn a2dp_handler(packet_type: u8, _channel: u16, packet: *mut u8, _size: u16) { unsafe {
    if packet_type != bt::HCI_EVENT_PACKET as u8 {
        return;
    }
    if bt::hci_event_packet_get_type(packet) as u32 != bt::HCI_EVENT_A2DP_META {
        return;
    }
    let a = app();
    match bt::hci_event_a2dp_meta_get_subevent_code(packet) as u32 {
        bt::A2DP_SUBEVENT_SIGNALING_CONNECTION_ESTABLISHED => {
            let status = bt::a2dp_subevent_signaling_connection_established_get_status(packet);
            if status as u32 != bt::ERROR_CODE_SUCCESS {
                println!("signaling connection failed, status {status:#04x}");
                // A 0x66 here means the bond-first step did not stick (key
                // dropped or never stored). The next cycle bonds again, so
                // just reconnect instead of bonding against the live ACL.
                schedule_reconnect();
                return;
            }
            a.a2dp_cid = bt::a2dp_subevent_signaling_connection_established_get_a2dp_cid(packet);
            a.sink_caps = None;
            a.ldac_remote_seid = None;
            a.state = State::Configuring;
            println!("signaling connected, cid {:#06x}", a.a2dp_cid);
        }
        bt::A2DP_SUBEVENT_SIGNALING_MEDIA_CODEC_OTHER_CAPABILITY => {
            let remote_seid =
                bt::a2dp_subevent_signaling_media_codec_other_capability_get_remote_seid(packet);
            let len = bt::a2dp_subevent_signaling_media_codec_other_capability_get_media_codec_information_len(
                packet,
            ) as usize;
            let info_ptr =
                bt::a2dp_subevent_signaling_media_codec_other_capability_get_media_codec_information(
                    packet,
                );
            let info = std::slice::from_raw_parts(info_ptr, len);
            if let Some(caps) = codec::parse_media_codec_info(info) {
                println!(
                    "sink seid {remote_seid} offers LDAC: freqs {:#04x}, channels {:#04x}",
                    caps.sampling_freqs, caps.channel_modes
                );
                a.sink_caps = Some(caps);
                a.ldac_remote_seid = Some(remote_seid);
            }
        }
        // DONE fires once per remote seid; COMPLETE is the only point at which
        // every endpoint has been queried
        bt::A2DP_SUBEVENT_SIGNALING_CAPABILITIES_COMPLETE => {
            let (caps, remote_seid) = match (a.sink_caps, a.ldac_remote_seid) {
                (Some(c), Some(s)) => (c, s),
                _ => {
                    println!("sink does not advertise LDAC - refusing to fall back silently");
                    bt::a2dp_source_disconnect(a.a2dp_cid);
                    a.state = State::Idle;
                    return;
                }
            };
            let chosen = match codec::choose_config(caps, a.format.sample_rate, a.format.channels) {
                Ok(c) => c,
                Err(e) => {
                    println!("cannot agree an LDAC configuration: {e}");
                    bt::a2dp_source_disconnect(a.a2dp_cid);
                    a.state = State::Idle;
                    return;
                }
            };
            a.set_config_info = codec::media_codec_info(chosen);
            println!(
                "configuring seid {remote_seid} for LDAC {} Hz, channels {:#04x}",
                a.format.sample_rate, chosen.channel_modes
            );
            let status = bt::a2dp_source_set_config_other(
                a.a2dp_cid,
                a.local_seid,
                remote_seid,
                a.set_config_info.as_ptr(),
                a.set_config_info.len() as u8,
            );
            if status as u32 != bt::ERROR_CODE_SUCCESS {
                println!("set configuration rejected, status {status:#04x}");
                schedule_reconnect();
            }
        }
        bt::A2DP_SUBEVENT_SIGNALING_MEDIA_CODEC_OTHER_CONFIGURATION => {
            let len = bt::a2dp_subevent_signaling_media_codec_other_configuration_get_media_codec_information_len(packet) as usize;
            let ptr = bt::a2dp_subevent_signaling_media_codec_other_configuration_get_media_codec_information(packet);
            let info = std::slice::from_raw_parts(ptr, len);
            a.negotiated = codec::parse_media_codec_info(info);
            if let Some(cfg) = a.negotiated {
                println!(
                    "sink accepted LDAC config: freq bit {:#04x}, channel bit {:#04x}",
                    cfg.sampling_freqs, cfg.channel_modes
                );
            }
        }
        bt::A2DP_SUBEVENT_STREAM_ESTABLISHED => {
            let status = bt::a2dp_subevent_stream_established_get_status(packet);
            if status as u32 != bt::ERROR_CODE_SUCCESS {
                println!("stream setup failed, status {status:#04x}");
                schedule_reconnect();
                return;
            }
            a.a2dp_cid = bt::a2dp_subevent_stream_established_get_a2dp_cid(packet);
            a.local_seid = bt::a2dp_subevent_stream_established_get_local_seid(packet);
            println!("stream established, starting");
            bt::a2dp_source_start_stream(a.a2dp_cid, a.local_seid);
        }
        bt::A2DP_SUBEVENT_STREAM_STARTED => {
            start_streaming();
        }
        bt::A2DP_SUBEVENT_STREAMING_CAN_SEND_MEDIA_PACKET_NOW => {
            send_packet();
        }
        bt::A2DP_SUBEVENT_STREAM_RELEASED => {
            println!("stream released");
            stop_streaming();
            schedule_reconnect();
        }
        bt::A2DP_SUBEVENT_SIGNALING_CONNECTION_RELEASED => {
            println!("signaling released");
            stop_streaming();
            schedule_reconnect();
        }
        _ => {}
    }
}}

unsafe fn start_streaming() { unsafe {
    let a = app();
    let max_payload = bt::a2dp_max_media_payload_size(a.a2dp_cid, a.local_seid) as usize;
    // ldacBT sizes its frames from the MTU it is given
    let enc = match Encoder::new(
        max_payload as i32,
        a.eqmid,
        a.format.channels,
        a.format.sample_rate,
    ) {
        Ok(e) => e,
        Err(code) => {
            println!("LDAC encoder init failed, ldac error {code}");
            bt::a2dp_source_disconnect(a.a2dp_cid);
            return;
        }
    };
    println!(
        "streaming: max payload {max_payload} B, initial bitrate {} kbps, eqmid {}",
        enc.bitrate(),
        enc.eqmid()
    );
    a.enc = Some(enc);
    a.packer = Some(Packer::new(max_payload));
    // the ring filled while we were connecting; that audio is stale
    a.audio.clear();
    a.rtp_timestamp = 0;
    a.pending_samples = 0;
    a.awaiting_can_send = false;
    a.primed = false;
    a.state = State::Streaming;
    a.streamed_once = true;
    a.stream_lost_ms = 0;

    bt::btstack_run_loop_remove_timer(&mut a.audio_timer);
    bt::btstack_run_loop_set_timer_handler(&mut a.audio_timer, Some(audio_timer_handler));
    bt::btstack_run_loop_set_timer(&mut a.audio_timer, AUDIO_TIMER_MS);
    bt::btstack_run_loop_add_timer(&mut a.audio_timer);
}}

unsafe fn stop_streaming() { unsafe {
    let a = app();
    bt::btstack_run_loop_remove_timer(&mut a.audio_timer);
    a.enc = None;
    a.packer = None;
    a.ready.clear();
    a.awaiting_can_send = false;
}}

unsafe extern "C" fn audio_timer_handler(_t: *mut bt::btstack_timer_source_t) { unsafe {
    let a = app();
    bt::btstack_run_loop_set_timer(&mut a.audio_timer, AUDIO_TIMER_MS);
    bt::btstack_run_loop_add_timer(&mut a.audio_timer);

    if a.state != State::Streaming {
        return;
    }

    let channels = a.format.channels as usize;
    let prime_samples = (a.format.sample_rate as usize * PRIME_MS as usize / 1000) * channels;
    if !a.primed {
        if a.audio.len() < prime_samples {
            return;
        }
        a.primed = true;
    }

    // never let a backlog monopolise the run loop: at most 4x realtime per tick
    let per_tick = (a.format.sample_rate as usize * AUDIO_TIMER_MS as usize / 1000)
        / encoder::FRAME_SAMPLES;
    let mut budget = (per_tick * 4).max(1);

    while a.audio.len() >= a.pcm.len() && budget > 0 && a.ready.len() < 32 {
        budget -= 1;
        a.audio.pop_padded(&mut a.pcm);
        let (out, payload) = match a.enc.as_mut().unwrap().encode(&a.pcm) {
            Ok(v) => v,
            Err(code) => {
                a.stats.encode_errors += 1;
                println!("ldac encode error {code}");
                return;
            }
        };
        a.stats.samples_consumed += (out.pcm_used / (channels * std::mem::size_of::<f32>())) as u64;
        a.stats.encode_calls += 1;
        if out.frames == 0 || out.bytes == 0 {
            continue;
        }
        let packer = a.packer.as_mut().unwrap();
        if packer.push_batch(payload, out.frames) == Push::Full {
            if let Some(done) = packer.flush() {
                let samples = std::mem::take(&mut a.pending_samples);
                a.ready.push_back((done, samples));
            }
            // the frame that did not fit opens the next packet
            assert_eq!(a.packer.as_mut().unwrap().push_batch(payload, out.frames), Push::Buffered);
        }
        // A transport frame represents 128 samples at 44.1/48 kHz and 256
        // at 88.2/96 kHz, independent of how many encode calls buffered it.
        a.pending_samples += out.frames as u32 * if a.format.sample_rate > 48_000 { 256 } else { 128 };
    }

    // Flush the last batch each tick so playback tails are not left queued.
    if let Some(done) = a.packer.as_mut().unwrap().flush() {
        a.ready.push_back((done, std::mem::take(&mut a.pending_samples)));
    }

    // Sending is paced by the link, not by this timer: ask for a slot as soon
    // as anything is queued and keep asking from the send handler.
    if !a.awaiting_can_send && !a.ready.is_empty() {
        a.awaiting_can_send = true;
        bt::a2dp_source_stream_endpoint_request_can_send_now(a.a2dp_cid, a.local_seid);
    }
}}

unsafe fn send_packet() { unsafe {
    let a = app();
    let (payload, samples) = match a.ready.pop_front() {
        Some(v) => v,
        None => {
            a.awaiting_can_send = false;
            return;
        }
    };
    let frames = payload[0] as u64;
    let status = bt::a2dp_source_stream_send_media_payload_rtp(
        a.a2dp_cid,
        a.local_seid,
        0,
        a.rtp_timestamp,
        payload.as_ptr() as *mut u8,
        payload.len() as u16,
    );
    if status as u32 != bt::ERROR_CODE_SUCCESS {
        a.stats.send_errors += 1;
    } else {
        a.rtp_timestamp = a.rtp_timestamp.wrapping_add(samples);
        a.stats.packets += 1;
        a.stats.frames += frames;
        a.stats.payload_bytes += payload.len() as u64;
    }

    if a.ready.is_empty() {
        a.awaiting_can_send = false;
    } else {
        bt::a2dp_source_stream_endpoint_request_can_send_now(a.a2dp_cid, a.local_seid);
    }
}}

unsafe extern "C" fn stats_timer_handler(_t: *mut bt::btstack_timer_source_t) { unsafe {
    let a = app();
    bt::btstack_run_loop_set_timer(&mut a.stats_timer, STATS_TIMER_MS);
    bt::btstack_run_loop_add_timer(&mut a.stats_timer);

    let now = bt::btstack_run_loop_get_time_ms() as u32;
    let elapsed = now.saturating_sub(a.stats.last_report_ms).max(1);
    a.stats.last_report_ms = now;

    let ring_ms = if a.format.sample_rate > 0 {
        a.audio.len() * 1000 / (a.format.sample_rate as usize * a.format.channels as usize)
    } else {
        0
    };
    let kbps = a.stats.payload_bytes * 8 / elapsed as u64;
    let (bitrate, eqmid) = match a.enc.as_ref() {
        Some(e) => (e.bitrate(), e.eqmid()),
        None => (0, -1),
    };
    // 1.00 means the encoder is consuming capture exactly as fast as it arrives
    let realtime_ratio = a.stats.samples_consumed as f64 * 1000.0
        / (a.format.sample_rate as f64 * elapsed as f64);
    let bytes_per_frame = if a.stats.frames > 0 {
        a.stats.payload_bytes / a.stats.frames
    } else {
        0
    };
    let frames_per_sec = a.stats.frames * 1000 / elapsed as u64;
    println!(
        "[{:?}] buffered {ring_ms} ms | rt {realtime_ratio:.2}x | {frames_per_sec} frame/s, {bytes_per_frame} B/frame | packets {} frames {} | wire {kbps} kbps | ldac {bitrate} kbps eqmid {eqmid} | over {} under {} | encerr {} senderr {} | reconnects {}",
        a.state,
        a.stats.packets,
        a.stats.frames,
        a.audio.overruns(),
        a.audio.underruns(),
        a.stats.encode_errors,
        a.stats.send_errors,
        a.stats.reconnects
    );
    a.stats.payload_bytes = 0;
    a.stats.samples_consumed = 0;
    a.stats.frames = 0;
    a.stats.encode_calls = 0;

    // steer LDAC quality from how much audio is queued
    if !a.abr {
    } else if let Some(enc) = a.enc.as_ref() {
        if ring_ms > 120 {
            enc.nudge_quality(-1);
        } else if ring_ms < 30 {
            enc.nudge_quality(1);
        }
    }

    // auto Windows fallback: only after a stream existed and then died.
    // Connecting/Configuring still counts as alive (bonding, setup); the
    // idle reconnect loop after a loss is what the timer measures.
    if a.fallback_after_s > 0 && a.streamed_once && !matches!(a.state, State::Streaming | State::Configuring) {
        if a.stream_lost_ms == 0 {
            a.stream_lost_ms = now;
        } else if now.saturating_sub(a.stream_lost_ms) > a.fallback_after_s * 1000 {
            if !auto_bt_fallback() {
                a.stream_lost_ms = now;
            }
            return;
        }
    } else {
        a.stream_lost_ms = 0;
    }
}}

/// The headset went away after streaming: hand the radio back to Windows
/// and exit. ldacmode.ps1 elevates itself, so this pops one UAC prompt and
/// the switch continues detached while we exit (exit code 4 = fallback).
/// Returns false when the switch script is missing, in which case the
/// caller keeps retrying instead of exiting.
unsafe fn auto_bt_fallback() -> bool { unsafe {
    println!("stream lost, handing radio back to Windows");
    let script = std::env::current_exe()
        .ok()
        .and_then(|exe| {
            exe.ancestors().map(|a| a.join("tools").join("ldacmode.ps1")).find(|p| p.exists())
        });
    match script {
        Some(path) => {
            let _ = std::process::Command::new("pwsh")
                .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
                .arg(&path)
                .arg("bt")
                .spawn();
            println!("Windows-mode switch launched, exiting");
            std::process::exit(4);
        }
        None => {
            println!("tools/ldacmode.ps1 not found next to the binary, staying put");
            false
        }
    }
}}


