# Limitations

## Blocking, by design of the platform

- **Windows loses Bluetooth entirely while this runs.** The radio is bound to
  WinUSB, so the Microsoft stack has no adapter. On this machine the CSR8510 is
  the only radio present, so there is no partial arrangement: it is either
  Windows' or this program's. `tools\ldacmode.ps1` makes that a one-command
  switch (3-11 s each way, measured over four consecutive round trips) but it
  cannot make both true at once.
- **Link keys are per stack.** A headset paired through `ldacsrc` may need
  re-pairing in Windows after switching back, and the reverse.

## Why both at once needs a kernel driver

The inbox driver binds through `microsoft_bluetooth_a2dp_src.inf` to
`BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}`, the per-paired-device PDO for
a remote A2DP sink. Replacing the function driver on that node is how a
third-party A2DP driver coexists with the rest of the stack, and it is what
Alternative A2DP Driver's per-device driver dropdown does.

That route is closed here by signing, not by code: Windows 10 1607 and later
load only kernel drivers signed by Microsoft through the Hardware Dev Center,
attestation signing requires an EV certificate plus a Partner Center account,
and `TESTSIGNING` cannot be enabled while Secure Boot is on. Secure Boot is on
here and stays on.

There is also no user-mode escape hatch on the Microsoft stack: Winsock's L2CAP
provider returns `WSAENETDOWN` for every `bind`/`connect`, and the Windows SDK
26100 WinRT Bluetooth surface exposes `RfcommDeviceService` with no
`L2capChannel` equivalent.
- **No third-party A2DP codec can be added to the Microsoft stack.** Nothing in
  Microsoft's Bluetooth driver documentation describes a registration point,
  and L2CAP is not reachable from user mode (measured; see ARCHITECTURE.md).
  This program exists because of that, not as a preference.

## Not implemented

- **No resampling.** The program requires the Windows shared mix format to
  already be 44.1, 48, 88.2 or 96 kHz and exits with an explanation otherwise.
  The default endpoint on this machine ran at 192 kHz, which LDAC cannot carry.
- **No SCO/HFP.** A2DP source only; there is no microphone path.
- **No AVRCP.** Volume keys and transport controls from the headset are not
  handled. BTstack supports it; this program does not register for it.
- **One sink at a time.**
- **Loopback only.** It captures whatever Windows is playing to the default
  render endpoint. It does not present itself as a separate audio device, so
  you cannot route individual apps to it.

## Unverified

- The LDAC media payload framing (one header byte, 4-bit frame count, then
  frames) follows bluez-alsa rather than the LDAC specification. It is
  confirmed working against one sink (WH-1000XM3), not against sinks generally.
- Confirmed on exactly one sink, at 96 kHz stereo, EQMID SQ (660 kbps), over a
  short indoor link. 44.1/48/88.2 kHz and the HQ/MQ modes are implemented but
  have not been listened to.
- ABR (`--abr`) is off by default and unproven; it walks EQMID from ring depth
  every 5 seconds, which is a crude control.

## Hardware

The adapter is a Bluetooth 4.0 Class 2 nano dongle (TP-Link UB400, CSR8510)
with a known history of A2DP dropouts on this machine even under the Microsoft
stack. LDAC at 909/990 kbps may not be sustainable over that link. The program
reports measured wire throughput every 5 seconds and steers EQMID from ring
depth rather than claiming a quality level.

## Licensing

BTstack (`third_party/btstack`) is licensed by BlueKitchen GmbH for personal,
non-commercial use only: "Any redistribution, use, or modification is done
solely for personal benefit and not for any commercial purpose or for monetary
gain." Commercial use requires a license from BlueKitchen. libldac is Apache
2.0 from Sony.
