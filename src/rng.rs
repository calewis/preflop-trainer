//! SplitMix64: a small deterministic PRNG. An item's seed must regenerate the
//! same item forever, so this module must never change its output sequence.

#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng { state: seed }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform integer in `0..n` (rejection sampling, so unbiased). `n` must be positive.
    pub fn below(&mut self, n: u64) -> u64 {
        assert!(n > 0, "Rng::below requires a positive bound");
        let zone = u64::MAX - (u64::MAX % n + 1) % n;
        loop {
            let v = self.next_u64();
            if v <= zone {
                return v % n;
            }
        }
    }

    /// Uniform integer in `lo..=hi`.
    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.below(hi - lo + 1)
    }

    /// True with probability `num / den`.
    pub fn chance(&mut self, num: u64, den: u64) -> bool {
        self.below(den) < num
    }

    pub fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len() as u64) as usize]
    }

    /// Fisher-Yates shuffle.
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.below(i as u64 + 1) as usize;
            items.swap(i, j);
        }
    }
}
