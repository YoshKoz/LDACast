/// The RTP media payload header carries the frame count in 4 bits.
pub const MAX_FRAMES_PER_PACKET: usize = 15;

/// Accumulates encoded LDAC frames into an A2DP media payload:
/// one header byte holding the frame count, followed by the frames.
pub struct Packer {
    max_payload: usize,
    buf: Vec<u8>,
    frames: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Push {
    Buffered,
    /// The frame did not fit; flush first, then offer it again.
    Full,
}

impl Packer {
    pub fn new(max_payload: usize) -> Packer {
        assert!(
            max_payload > 1,
            "max payload must leave room for the header"
        );
        Packer {
            max_payload,
            buf: vec![0u8],
            frames: 0,
        }
    }

    pub fn push_batch(&mut self, frame: &[u8], frames: usize) -> Push {
        assert!(frames > 0 && frames <= MAX_FRAMES_PER_PACKET);
        assert!(
            frame.len() < self.max_payload,
            "LDAC batch exceeds media MTU"
        );
        if self.frames + frames > MAX_FRAMES_PER_PACKET
            || self.buf.len() + frame.len() > self.max_payload
        {
            return Push::Full;
        }
        self.buf.extend_from_slice(frame);
        self.frames += frames;
        Push::Buffered
    }

    /// Returns the payload and resets the packer. `None` when nothing is queued.
    pub fn flush(&mut self) -> Option<Vec<u8>> {
        if self.frames == 0 {
            return None;
        }
        self.buf[0] = self.frames as u8;
        let out = std::mem::replace(&mut self.buf, vec![0u8]);
        self.frames = 0;
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Packer {
        fn push(&mut self, frame: &[u8]) -> Push {
            self.push_batch(frame, 1)
        }
    }

    #[test]
    fn counts_frames_inside_encoder_batches() {
        let mut p = Packer::new(100);
        assert_eq!(p.push_batch(&[0; 20], 6), Push::Buffered);
        assert_eq!(p.push_batch(&[0; 20], 6), Push::Buffered);
        assert_eq!(p.push_batch(&[0; 20], 6), Push::Full);
        assert_eq!(p.flush().unwrap()[0], 12);
        assert_eq!(p.push_batch(&[0; 20], 6), Push::Buffered);
        assert_eq!(p.flush().unwrap()[0], 6);
    }

    #[test]
    fn header_counts_frames() {
        let mut p = Packer::new(100);
        assert_eq!(p.push(&[1, 2, 3]), Push::Buffered);
        assert_eq!(p.push(&[4, 5, 6]), Push::Buffered);
        let out = p.flush().unwrap();
        assert_eq!(out, vec![2, 1, 2, 3, 4, 5, 6]);
        assert!(p.flush().is_none());
    }

    #[test]
    fn refuses_to_exceed_max_payload() {
        let mut p = Packer::new(8);
        assert_eq!(p.push(&[0; 5]), Push::Buffered);
        assert_eq!(p.push(&[0; 5]), Push::Full);
        assert_eq!(p.flush().unwrap().len(), 6);
    }

    #[test]
    fn frame_count_never_overflows_the_nibble() {
        let mut p = Packer::new(4096);
        for _ in 0..MAX_FRAMES_PER_PACKET {
            assert_eq!(p.push(&[0; 2]), Push::Buffered);
        }
        assert_eq!(p.push(&[0; 2]), Push::Full);
        let out = p.flush().unwrap();
        assert_eq!(out[0], 15);
        assert_eq!(out[0] & 0xf0, 0);
    }
}
