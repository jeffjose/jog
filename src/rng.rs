use std::time::{SystemTime, UNIX_EPOCH};

/// A small xorshift generator; shuffling a playlist needs nothing stronger.
pub struct Rng(u64);

impl Rng {
    pub fn from_clock() -> Self {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
        Rng(nanos ^ (u64::from(std::process::id()) << 32) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// A value in `0..bound`. `bound` must be non-zero.
    pub fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }

    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            items.swap(i, self.below(i + 1));
        }
    }
}
