// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Random numbers from a seed. The same seed gives the same numbers on
//! every machine: the generator is integer arithmetic, and the normal
//! deviates are made with the deterministic functions of `dspkit::math`.

use dspkit::math;

/// A generator of random numbers: xorshift64* (Vigna, "An experimental
/// exploration of Marsaglia's xorshift generators, scrambled", 2016).
#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
    spare: Option<f64>,
}

impl Rng {
    /// A generator for a seed. Every seed is good, zero too.
    pub fn new(seed: u64) -> Rng {
        // SplitMix64 spreads the seed over the state, and keeps a seed of
        // zero from being the one state the generator cannot leave.
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        Rng {
            state: if z == 0 { 0x9E37_79B9_7F4A_7C15 } else { z },
            spare: None,
        }
    }

    /// The next 64 bits.
    pub fn next_u64(&mut self) -> u64 {
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        self.state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A number above zero and below one, evenly spread.
    pub fn uniform(&mut self) -> f64 {
        // 53 bits, moved off zero by half a step.
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    /// Whether an event of this probability happens.
    pub fn chance(&mut self, probability: f64) -> bool {
        self.uniform() < probability
    }

    /// A normal deviate of mean zero and variance one, by the method of
    /// Box and Muller.
    pub fn gaussian(&mut self) -> f64 {
        if let Some(spare) = self.spare.take() {
            return spare;
        }
        let radius = (-2.0 * math::ln(self.uniform())).sqrt();
        let angle = math::TAU * self.uniform();
        self.spare = Some(radius * math::sin(angle));
        radius * math::cos(angle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seed_gives_the_same_numbers_again() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        let mut c = Rng::new(43);
        let mut same = 0;
        for _ in 0..1000 {
            let x = a.next_u64();
            assert_eq!(x, b.next_u64());
            if x == c.next_u64() {
                same += 1;
            }
        }
        assert_eq!(same, 0);
        // A seed of zero works.
        let mut zero = Rng::new(0);
        assert_ne!(zero.next_u64(), zero.next_u64());
    }

    #[test]
    fn the_deviates_are_normal() {
        let mut rng = Rng::new(1);
        let n = 400_000;
        let (mut sum, mut squares, mut fourth) = (0.0, 0.0, 0.0);
        let mut beyond_two = 0;
        for _ in 0..n {
            let x = rng.gaussian();
            sum += x;
            squares += x * x;
            fourth += x * x * x * x;
            if x.abs() > 2.0 {
                beyond_two += 1;
            }
        }
        let n = n as f64;
        assert!((sum / n).abs() < 0.01);
        assert!((squares / n - 1.0).abs() < 0.01);
        // The fourth moment of a normal deviate is 3.
        assert!((fourth / n - 3.0).abs() < 0.05);
        // 4.55 percent are more than two deviations out.
        assert!((f64::from(beyond_two) / n - 0.0455).abs() < 0.002);
    }

    #[test]
    fn the_uniform_numbers_are_inside_their_range_and_even() {
        let mut rng = Rng::new(9);
        let mut below_half = 0;
        let mut hits = 0;
        for _ in 0..100_000 {
            let x = rng.uniform();
            assert!(x > 0.0 && x < 1.0);
            if x < 0.5 {
                below_half += 1;
            }
            if rng.chance(0.1) {
                hits += 1;
            }
        }
        assert!((49_000..51_000).contains(&below_half));
        assert!((9_500..10_500).contains(&hits));
    }
}
