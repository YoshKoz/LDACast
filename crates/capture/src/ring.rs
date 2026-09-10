use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

struct Shared {
    buf: Box<[UnsafeCell<f32>]>,
    mask: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
    overruns: AtomicU64,
    underruns: AtomicU64,
}

unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}

/// Single-producer single-consumer float ring. The capture thread only ever
/// touches `head`; the protocol thread only ever touches `tail`.
pub struct Producer(Arc<Shared>);
pub struct Consumer(Arc<Shared>);

pub fn ring(capacity_samples: usize) -> (Producer, Consumer) {
    let cap = capacity_samples.next_power_of_two();
    let shared = Arc::new(Shared {
        buf: (0..cap).map(|_| UnsafeCell::new(0.0)).collect(),
        mask: cap - 1,
        head: AtomicUsize::new(0),
        tail: AtomicUsize::new(0),
        overruns: AtomicU64::new(0),
        underruns: AtomicU64::new(0),
    });
    (Producer(shared.clone()), Consumer(shared))
}

impl Shared {
    fn len(&self) -> usize {
        self.head.load(Ordering::Acquire).wrapping_sub(self.tail.load(Ordering::Acquire))
    }
}

impl Producer {
    /// Writes available samples, dropping new data when the ring is full.
    /// Only the consumer may advance tail; overwriting unread data races it.
    pub fn push(&self, src: &[f32]) {
        let s = &self.0;
        let cap = s.mask + 1;
        let free = cap - s.len();
        let n = src.len().min(free);
        s.overruns.fetch_add((src.len() - n) as u64, Ordering::Relaxed);
        let head = s.head.load(Ordering::Relaxed);
        for (i, v) in src[..n].iter().enumerate() {
            unsafe { *s.buf[head.wrapping_add(i) & s.mask].get() = *v; }
        }
        s.head.store(head.wrapping_add(n), Ordering::Release);
    }

    pub fn overruns(&self) -> u64 {
        self.0.overruns.load(Ordering::Relaxed)
    }
}

impl Consumer {
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Fills `dst` completely, padding with silence and counting an underrun if
    /// the producer has not kept up.
    pub fn pop_padded(&self, dst: &mut [f32]) -> bool {
        let s = &self.0;
        let avail = s.len();
        let n = avail.min(dst.len());
        let tail = s.tail.load(Ordering::Relaxed);
        for i in 0..n {
            dst[i] = unsafe { *s.buf[tail.wrapping_add(i) & s.mask].get() };
        }
        s.tail.store(tail.wrapping_add(n), Ordering::Release);
        if n < dst.len() {
            dst[n..].fill(0.0);
            s.underruns.fetch_add((dst.len() - n) as u64, Ordering::Relaxed);
            return false;
        }
        true
    }

    /// Drops everything queued so streaming starts from live audio.
    pub fn clear(&self) {
        let s = &self.0;
        s.tail.store(s.head.load(Ordering::Acquire), Ordering::Release);
        s.overruns.store(0, Ordering::Relaxed);
        s.underruns.store(0, Ordering::Relaxed);
    }

    pub fn underruns(&self) -> u64 {
        self.0.underruns.load(Ordering::Relaxed)
    }

    pub fn overruns(&self) -> u64 {
        self.0.overruns.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_wraparound() {
        let (p, c) = ring(8);
        let mut out = [0.0f32; 6];
        for round in 0..10 {
            let src: Vec<f32> = (0..6).map(|i| (round * 6 + i) as f32).collect();
            p.push(&src);
            assert!(c.pop_padded(&mut out));
            assert_eq!(out.to_vec(), src);
        }
        assert_eq!(c.overruns(), 0);
        assert_eq!(c.underruns(), 0);
    }

    #[test]
    fn overrun_preserves_unread_samples() {
        let (p, c) = ring(4);
        p.push(&[1.0, 2.0, 3.0, 4.0]);
        p.push(&[5.0, 6.0]);
        assert_eq!(c.overruns(), 2);
        let mut out = [0.0f32; 4];
        assert!(c.pop_padded(&mut out));
        assert_eq!(out, [1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn underrun_pads_with_silence() {
        let (p, c) = ring(8);
        p.push(&[1.0, 2.0]);
        let mut out = [9.0f32; 4];
        assert!(!c.pop_padded(&mut out));
        assert_eq!(out, [1.0, 2.0, 0.0, 0.0]);
        assert_eq!(c.underruns(), 2);
    }
}
