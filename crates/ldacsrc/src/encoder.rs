use ldac_sys as sys;

/// Input samples per channel that ldacBT_encode() consumes per call.
pub const FRAME_SAMPLES: usize = sys::LDACBT_ENC_LSU as usize;
pub const MAX_FRAME_BYTES: usize = sys::LDACBT_MAX_NBYTES as usize;

pub const EQMID_HQ: i32 = sys::LDACBT_EQMID_HQ as i32;
pub const EQMID_SQ: i32 = sys::LDACBT_EQMID_SQ as i32;
pub const EQMID_MQ: i32 = sys::LDACBT_EQMID_MQ as i32;

pub struct Encoder {
    handle: sys::HANDLE_LDAC_BT,
    pub channels: usize,
    scratch: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct Encoded {
    pub bytes: usize,
    pub frames: usize,
    pub pcm_used: usize,
}

impl Encoder {
    pub fn new(mtu: i32, eqmid: i32, channels: u16, sample_rate: u32) -> Result<Encoder, i32> {
        let channel_mode = match channels {
            1 => sys::LDACBT_CHANNEL_MODE_MONO as i32,
            _ => sys::LDACBT_CHANNEL_MODE_STEREO as i32,
        };
        unsafe {
            let handle = sys::ldacBT_get_handle();
            if handle.is_null() {
                return Err(-1);
            }
            let rc = sys::ldacBT_init_handle_encode(
                handle,
                mtu,
                eqmid,
                channel_mode,
                sys::LDACBT_SMPL_FMT_T_LDACBT_SMPL_FMT_F32,
                sample_rate as i32,
            );
            if rc != 0 {
                let err = sys::ldacBT_get_error_code(handle);
                sys::ldacBT_free_handle(handle);
                return Err(err);
            }
            Ok(Encoder {
                handle,
                channels: channels as usize,
                scratch: vec![0u8; MAX_FRAME_BYTES],
            })
        }
    }

    /// `pcm` must hold exactly FRAME_SAMPLES interleaved frames.
    pub fn encode(&mut self, pcm: &[f32]) -> Result<(Encoded, &[u8]), i32> {
        assert_eq!(pcm.len(), FRAME_SAMPLES * self.channels);
        let mut used: i32 = 0;
        let mut size: i32 = 0;
        let mut frames: i32 = 0;
        let rc = unsafe {
            sys::ldacBT_encode(
                self.handle,
                pcm.as_ptr() as *mut std::ffi::c_void,
                &mut used,
                self.scratch.as_mut_ptr(),
                &mut size,
                &mut frames,
            )
        };
        if rc != 0 {
            return Err(unsafe { sys::ldacBT_get_error_code(self.handle) });
        }
        let out = Encoded {
            bytes: size as usize,
            frames: frames as usize,
            pcm_used: used as usize,
        };
        Ok((out, &self.scratch[..size as usize]))
    }

    pub fn bitrate(&self) -> i32 {
        unsafe { sys::ldacBT_get_bitrate(self.handle) }
    }

    pub fn eqmid(&self) -> i32 {
        unsafe { sys::ldacBT_get_eqmid(self.handle) }
    }

    /// priority > 0 raises quality, < 0 favours connection stability.
    pub fn nudge_quality(&self, priority: i32) {
        unsafe { sys::ldacBT_alter_eqmid_priority(self.handle, priority) };
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe {
            sys::ldacBT_close_handle(self.handle);
            sys::ldacBT_free_handle(self.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_silence_to_frames() {
        let mut enc = Encoder::new(679, EQMID_SQ, 2, 48000).expect("init");
        let pcm = vec![0.0f32; FRAME_SAMPLES * 2];
        let mut produced = 0;
        // the encoder buffers internally, so a few calls may yield nothing
        for _ in 0..8 {
            let (out, payload) = enc.encode(&pcm).expect("encode");
            assert_eq!(payload.len(), out.bytes);
            produced += out.frames;
        }
        assert!(produced > 0, "no LDAC frames produced in 8 calls");
        assert!(enc.bitrate() > 0);
    }

    #[test]
    fn rejects_unsupported_sample_rate() {
        assert!(Encoder::new(679, EQMID_SQ, 2, 22050).is_err());
    }

    #[test]
    fn packet_counts_match_real_encoder_batches() {
        use crate::packer::{Packer, Push};
        for rate in [44100, 48000, 88200, 96000] {
            for quality in [EQMID_HQ, EQMID_SQ, EQMID_MQ] {
                let mut enc = Encoder::new(883, quality, 2, rate).unwrap();
                let pcm = vec![0.0f32; FRAME_SAMPLES * 2];
                let mut packer = Packer::new(883);
                let mut frames = 0;
                for _ in 0..32 {
                    let (out, payload) = enc.encode(&pcm).unwrap();
                    assert_eq!(out.pcm_used, pcm.len() * size_of::<f32>());
                    if out.frames == 0 { continue; }
                    assert_eq!(packer.push_batch(payload, out.frames), Push::Buffered);
                    let packet = packer.flush().unwrap();
                    assert_eq!(packet[0] as usize, out.frames);
                    assert_eq!(&packet[1..], payload);
                    assert!(packet.len() <= 883);
                    frames += out.frames;
                }
                assert!(frames > 0);
                let samples = frames * if rate > 48000 { 256 } else { 128 };
                assert!(samples <= 32 * FRAME_SAMPLES);
                assert!(samples >= 24 * FRAME_SAMPLES);
            }
        }
    }
}
