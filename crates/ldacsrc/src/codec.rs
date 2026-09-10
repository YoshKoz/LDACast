pub const SONY_VENDOR_ID: u32 = 0x0000_012D;
pub const LDAC_CODEC_ID: u16 = 0x00AA;

pub const FREQ_44100: u8 = 1 << 5;
pub const FREQ_48000: u8 = 1 << 4;
pub const FREQ_88200: u8 = 1 << 3;
pub const FREQ_96000: u8 = 1 << 2;

pub const CHAN_MONO: u8 = 1 << 2;
pub const CHAN_DUAL: u8 = 1 << 1;
pub const CHAN_STEREO: u8 = 1 << 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LdacCaps {
    pub sampling_freqs: u8,
    pub channel_modes: u8,
}

/// Non-A2DP media codec information: vendor id (LE32), codec id (LE16),
/// sampling frequency bitmap, channel mode bitmap.
pub fn media_codec_info(caps: LdacCaps) -> [u8; 8] {
    let v = SONY_VENDOR_ID.to_le_bytes();
    let c = LDAC_CODEC_ID.to_le_bytes();
    [v[0], v[1], v[2], v[3], c[0], c[1], caps.sampling_freqs, caps.channel_modes]
}

pub fn parse_media_codec_info(info: &[u8]) -> Option<LdacCaps> {
    if info.len() < 8 {
        return None;
    }
    let vendor = u32::from_le_bytes([info[0], info[1], info[2], info[3]]);
    let codec = u16::from_le_bytes([info[4], info[5]]);
    if vendor != SONY_VENDOR_ID || codec != LDAC_CODEC_ID {
        return None;
    }
    Some(LdacCaps { sampling_freqs: info[6], channel_modes: info[7] })
}

pub fn freq_bit(rate: u32) -> Option<u8> {
    match rate {
        44100 => Some(FREQ_44100),
        48000 => Some(FREQ_48000),
        88200 => Some(FREQ_88200),
        96000 => Some(FREQ_96000),
        _ => None,
    }
}

/// Picks the configuration to send in SET_CONFIGURATION: exactly one frequency
/// bit and one channel mode bit, both of which the sink offered.
pub fn choose_config(sink: LdacCaps, rate: u32, channels: u16) -> Result<LdacCaps, String> {
    let freq = freq_bit(rate).ok_or_else(|| format!("LDAC does not carry {rate} Hz"))?;
    if sink.sampling_freqs & freq == 0 {
        return Err(format!(
            "sink does not accept {rate} Hz (offered bitmap {:#04x})",
            sink.sampling_freqs
        ));
    }
    let chan = match channels {
        1 => CHAN_MONO,
        2 => CHAN_STEREO,
        n => return Err(format!("{n} capture channels not representable in LDAC")),
    };
    if sink.channel_modes & chan == 0 {
        return Err(format!(
            "sink does not accept {channels} channel(s) (offered bitmap {:#04x})",
            sink.channel_modes
        ));
    }
    Ok(LdacCaps { sampling_freqs: freq, channel_modes: chan })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bytes captured from a WH-1000XM3 AVDTP GET_ALL_CAPABILITIES response,
    /// seid 5: 2d 01 00 00 aa 00 3c 07
    const XM3_INFO: [u8; 8] = [0x2d, 0x01, 0x00, 0x00, 0xaa, 0x00, 0x3c, 0x07];

    #[test]
    fn parses_real_sink_capability() {
        let caps = parse_media_codec_info(&XM3_INFO).unwrap();
        assert_eq!(caps.sampling_freqs, FREQ_44100 | FREQ_48000 | FREQ_88200 | FREQ_96000);
        assert_eq!(caps.channel_modes, CHAN_MONO | CHAN_DUAL | CHAN_STEREO);
    }

    #[test]
    fn info_roundtrips() {
        let caps = parse_media_codec_info(&XM3_INFO).unwrap();
        assert_eq!(media_codec_info(caps), XM3_INFO);
    }

    #[test]
    fn rejects_other_vendors() {
        let mut info = XM3_INFO;
        info[0] = 0x4f; // apt Ltd
        assert!(parse_media_codec_info(&info).is_none());
    }

    #[test]
    fn chooses_single_bits() {
        let sink = parse_media_codec_info(&XM3_INFO).unwrap();
        let cfg = choose_config(sink, 48000, 2).unwrap();
        assert_eq!(cfg.sampling_freqs, FREQ_48000);
        assert_eq!(cfg.channel_modes, CHAN_STEREO);
    }

    #[test]
    fn rejects_rate_the_sink_did_not_offer() {
        let sink = LdacCaps { sampling_freqs: FREQ_44100, channel_modes: CHAN_STEREO };
        assert!(choose_config(sink, 96000, 2).is_err());
    }

    #[test]
    fn rejects_rate_ldac_cannot_carry() {
        let sink = parse_media_codec_info(&XM3_INFO).unwrap();
        assert!(choose_config(sink, 192_000, 2).is_err());
    }
}
