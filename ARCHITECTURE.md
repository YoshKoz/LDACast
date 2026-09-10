# Architecture

## What this is

A user-mode LDAC A2DP source for Windows. It captures the system audio mix with
WASAPI loopback, encodes it with Sony's libldac, and streams it over A2DP to a
Bluetooth sink, driving the radio itself over WinUSB.

```
WASAPI loopback -> SPSC ring -> libldac -> RTP payload -> AVDTP/L2CAP -> HCI -> WinUSB -> CSR8510
     capture thread          |          BTstack run-loop thread
```

The two threads share nothing but the ring buffer. The capture thread never
calls into BTstack, and the protocol thread never blocks on the audio device.

## Verified on this machine

Everything in this section was measured here, not taken from documentation.

### The Microsoft stack cannot carry this

`MSAFD L2CAP [Bluetooth]` is a registered Winsock provider on Windows 11
26200.9168 (`af 32, socktype 1, proto 0x100`), and `socket()` on it returns a
valid handle. Every operation past that fails:

```
sizeof SOCKADDR_BTH 30
L2CAP bind PSM 0x1001     -> -1 WSA 10050
L2CAP bind PSM 0x19       -> -1 WSA 10050   (AVDTP)
L2CAP bind PSM 0xffffffff -> -1 WSA 10050
L2CAP connect             -> -1 WSA 10050
RFCOMM bind ANY           ->  0 WSA 0
RFCOMM listen             ->  0 WSA 0
```

Same process, same 30-byte `SOCKADDR_BTH`, same second: RFCOMM works, L2CAP
returns `WSAENETDOWN` for everything. AVDTP lives on PSM 0x0019, so A2DP cannot
be driven from user mode through the inbox stack.

Microsoft's own codec table (`bluetooth-classic-audio`, ms.date 2024-11-14)
lists SBC, aptX Classic, AAC and aptX Adaptive for A2DP on Windows 11 - no
LDAC - and documents no registration point for a third-party source codec.

### Taking the radio

The dongle is `USB\VID_0A12&PID_0001\6&1B2B7D3C&0&1`, "Cambridge Silicon Radio
Ltd.". Its original binding is recorded in `docs/radio-original-driver.txt`:
service `BTHUSB`, `bth.inf`, driver `10.0.26100.8972`.

After Zadig installs WinUSB the device moves to class `USBDevice`, service
`WinUSB`, provider `libwdi`, `oem14.inf`.

**A driver swap alone is not enough.** Immediately after Zadig, the registry
said `WinUSB` while the device still had its Bluetooth children
(`BTH\MS_RFCOMM`, `BTH\MS_BTHBRB`, ...) and `WinUsb_Initialize` failed with
`ERROR_NOT_SUPPORTED` (50); libwdi's own interface GUID did not exist. The old
stack was still loaded. `pnputil /remove-device` reported "System reboot is
needed", and only a physical unplug/replug rebuilt the devnode. After the
replug the device has no children and both interface paths initialise:

```
{a5dcbf10-6530-11d2-901f-00c04fb951ed} CreateFile ok WinUsb_Initialize True err 0
{e1ad5836-d63c-4a9a-936d-bc946594c92b} CreateFile ok WinUsb_Initialize True err 0
```

### The sink

A WH-1000XM3 answered AVDTP DISCOVER with five sink endpoints. GET_ALL_CAPABILITIES
per seid gave SBC (1), AAC (2), aptX (3), LDAC (5), aptX HD (6). The raw LDAC
capability bytes for seid 5, straight off the wire:

```
07 0a 00 ff 2d 01 00 00 aa 00 3c 07
```

which is service category 7 (Media Codec), LOSC 10, media type 0x00 (audio),
codec type 0xFF (Non-A2DP), vendor `0x0000012D`, codec `0x00AA`, sampling
frequency bitmap `0x3C`, channel mode bitmap `0x07`. Those last two decode
against bluez-alsa's definitions as 44.1/48/88.2/96 kHz and mono+dual+stereo.
`crates/ldacsrc/src/codec.rs` parses exactly these bytes in a unit test.

An SBC stream to the same headphones was established and produced audible
output, confirming the whole path end to end before LDAC was attempted. LDAC
was then negotiated and streamed to the same headphones at 96 kHz stereo and
confirmed clean by listening, with the transport running at the encoder's rate:

```
[Streaming] buffered 0 ms | rt 1.00x | 125 frame/s, 661 B/frame | wire 661 kbps
            | ldac 660 kbps eqmid 1 | over 0 under 0 | encerr 0 senderr 0
```

### Send pacing is the thing that makes or breaks it

Two defects produced audible distortion before this was right, and both were
found from counters rather than by ear:

1. When a packet filled, the frame that did not fit was pushed back into the
   still-full packer and silently dropped. With 661 B frames in an 883 B
   payload only one frame fits, so every second frame was lost.
2. Requesting a can-send slot only once per 10 ms timer tick capped throughput
   at ~64 packets/s where 125/s are needed. `rt 0.52x` said so directly: the
   encoder consumed half of real time, the ring stayed full, and the overrun
   counter climbed forever.

Sending is now paced by the link: finished payloads go on a queue, the timer
asks for a slot whenever the queue is non-empty, and the send handler asks
again immediately if more are waiting. `rt` and the overrun counter are the
signals to watch - `rt` below 1.00 with rising overruns means audio is being
thrown away.

## Components

| Crate | Responsibility |
| --- | --- |
| `btstack-sys` | BTstack built from source, bindgen FFI |
| `ldac-sys` | libldac built from source, bindgen FFI |
| `capture` | WASAPI loopback thread and the SPSC ring |
| `ldacsrc` | A2DP/AVDTP logic, encode, packetise, reconnect |

`btstack-sys` compiles the same source set as BTstack's `port/windows-winusb`
CMake target, minus `main.c` (the Rust binary provides its own entry point) and
`le_device_db_memory.c`. BTstack's event accessors are `static inline`, so
bindgen's `wrap_static_fns` generates C thunks that are compiled alongside.

## Media framing

An A2DP media payload is one header byte followed by concatenated LDAC frames.
The header's low nibble is the frame count, so a packet carries at most 15
frames; `Packer` enforces both that and the AVDTP MTU from
`a2dp_max_media_payload_size()`. The RTP timestamp advances by the number of
PCM samples consumed for the frames in the packet.

## Still assumed, not verified

- The payload layout is taken from bluez-alsa's `rtp_media_header_t` and
  `a2dp-ldac.c`, not from the LDAC specification. A WH-1000XM3 decodes it
  cleanly, which is evidence but not proof for other sinks.
- That `ldacBT_init_handle_encode`'s `mtu` argument should be the AVDTP media
  payload size. libldac's header says it is "MTU size of AVDTP Transport
  Channel"; the app passes `a2dp_max_media_payload_size()`, and the resulting
  661 B frames sit inside the 883 B payload.
- Whether `ldacBT_alter_eqmid_priority` is a useful ABR control on this link.
  It is off by default (`--abr`); during the first tests it walked EQMID down
  every 5 seconds while the real problem was send pacing.
- The RTP timestamp is advanced by the samples accumulated when a packet is
  closed, so a frame that spills into the next packet is accounted one packet
  late. No sink has objected.
- Whether the CSR8510 sustains LDAC bitrates at all - see LIMITATIONS.md.
