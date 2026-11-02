//! A bounded two-word snapshot for a serialized settings writer. Every payload access is atomic. Readers make one
//! attempt and skip an effect if its writer was paused or changed the version; they never spin on a game thread.
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};

#[repr(C)]
pub(crate) struct Wide {
    version: AtomicU64,
    low: AtomicU64,
    high: AtomicU64,
}
impl Wide {
    pub const fn new() -> Self {
        Self {
            version: AtomicU64::new(0),
            low: AtomicU64::new(0),
            high: AtomicU64::new(0),
        }
    }
    /// The gate's serialized control owner is the only writer.
    pub fn set(&self, value: u128) {
        self.version.fetch_add(1, SeqCst);
        self.low.store(value as u64, SeqCst);
        self.high.store((value >> 64) as u64, SeqCst);
        self.version.fetch_add(1, SeqCst);
    }
    pub fn get(&self) -> Option<u128> {
        let version = self.version.load(SeqCst);
        if version & 1 != 0 {
            return None;
        }
        let low = self.low.load(SeqCst);
        let high = self.high.load(SeqCst);
        (self.version.load(SeqCst) == version).then_some(u128::from(low) | (u128::from(high) << 64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paused_writer_never_blocks_and_concurrent_snapshots_never_mix_words() {
        let settings = Wide::new();
        settings.version.store(1, SeqCst);
        settings.low.store(123, SeqCst);
        assert_eq!(settings.get(), None);
        settings.version.store(0, SeqCst);
        settings.set(0);
        std::thread::scope(|scope| {
            let writer = scope.spawn(|| {
                for value in 1..100_000u64 {
                    settings.set(u128::from(value) | (u128::from(value) << 64));
                }
            });
            while !writer.is_finished() {
                if let Some(value) = settings.get() {
                    assert_eq!(value as u64, (value >> 64) as u64);
                }
            }
        });
    }
}
