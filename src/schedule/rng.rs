//! A tiny seeded random number generator, written out here so a schedule is the
//! same on every machine and every Rust version. The `rand` crate and the
//! standard library's hashers make no such promise.

/// SplitMix64, the 64-bit generator from Steele, Lea and Flood (2014).
pub(super) struct SplitMix64(u64);

impl SplitMix64 {
    pub(super) fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub(super) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `0..bound`. `bound` must not be zero.
    fn below(&mut self, bound: usize) -> usize {
        // Multiply-and-shift keeps the result in range without a division and
        // with a bias too small to matter for a TV schedule.
        ((u128::from(self.next_u64()) * bound as u128) >> 64) as usize
    }
}

/// Folds the parts of a shuffle's identity into one seed.
pub(super) fn seed(channel: u32, local_date_days: i32, salt: u32) -> u64 {
    let mut rng = SplitMix64::new(0);
    for part in [
        u64::from(channel),
        i64::from(local_date_days) as u64,
        u64::from(salt),
    ] {
        rng.0 ^= part;
        rng.next_u64();
    }
    rng.next_u64()
}

/// A Fisher-Yates shuffle of `0..len`.
pub(super) fn shuffled_indices(len: usize, rng: &mut SplitMix64) -> Vec<usize> {
    let mut order: Vec<usize> = (0..len).collect();
    for i in (1..len).rev() {
        order.swap(i, rng.below(i + 1));
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitmix64_matches_the_reference_outputs_for_seed_zero() {
        let mut rng = SplitMix64::new(0);
        assert_eq!(rng.next_u64(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(rng.next_u64(), 0x6E78_9E6A_A1B9_65F4);
        assert_eq!(rng.next_u64(), 0x06C4_5D18_8009_454F);
    }

    #[test]
    fn a_shuffle_is_a_permutation_and_repeats_for_the_same_seed() {
        let first = shuffled_indices(9, &mut SplitMix64::new(seed(3, 20_400, 0)));
        let again = shuffled_indices(9, &mut SplitMix64::new(seed(3, 20_400, 0)));
        let mut sorted = first.clone();
        sorted.sort_unstable();
        assert_eq!(first, again);
        assert_eq!(sorted, (0..9).collect::<Vec<_>>());
    }

    #[test]
    fn the_seed_depends_on_every_part() {
        let base = seed(3, 20_400, 0);
        assert_ne!(base, seed(4, 20_400, 0));
        assert_ne!(base, seed(3, 20_401, 0));
        assert_ne!(base, seed(3, 20_400, 1));
    }
}
