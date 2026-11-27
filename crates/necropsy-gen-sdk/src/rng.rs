// SPDX-License-Identifier: AGPL-3.0-or-later
//! xoshiro256** seeded with the four seed words. The site derives the seed with
//! HMAC-SHA-256, so the words are already uniform; this only stretches them.

pub struct Rng {
    s: [u64; 4],
}

impl Rng {
    pub fn new(seed: [u64; 4]) -> Self {
        // An all-zero state never leaves zero; nudge it.
        let s = if seed == [0; 4] { [1, 2, 3, 4] } else { seed };
        let mut r = Self { s };
        // Warm up, so similar seeds diverge at once.
        for _ in 0..16 {
            r.next_u64();
        }
        r
    }

    pub fn next_u64(&mut self) -> u64 {
        let result = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    /// A number in `0..n`, without modulo bias. `n` must not be 0.
    pub fn below(&mut self, n: u64) -> u64 {
        assert!(n > 0, "below(0)");
        let zone = u64::MAX - (u64::MAX % n);
        loop {
            let v = self.next_u64();
            if v < zone {
                return v % n;
            }
        }
    }

    /// A number in `lo..=hi`.
    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.below(hi - lo + 1)
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }

    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.below(i as u64 + 1) as usize;
            items.swap(i, j);
        }
    }

    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next_u64() as u8).collect()
    }

    /// `n` characters from `alphabet`.
    pub fn string(&mut self, alphabet: &str, n: usize) -> String {
        let chars: Vec<char> = alphabet.chars().collect();
        (0..n).map(|_| *self.pick(&chars)).collect()
    }

    /// `n` lowercase hex digits.
    pub fn hex(&mut self, n: usize) -> String {
        self.string("0123456789abcdef", n)
    }

    /// A flag such as `pebble{k3x9q2mfa7}`: a prefix and `n` random characters
    /// from lowercase letters and digits.
    pub fn flag(&mut self, prefix: &str, n: usize) -> String {
        format!(
            "{prefix}{{{}}}",
            self.string("abcdefghijkmnpqrstuvwxyz23456789", n)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_in_range() {
        let mut a = Rng::new([9, 9, 9, 9]);
        let mut b = Rng::new([9, 9, 9, 9]);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        for _ in 0..1000 {
            let v = a.range(3, 7);
            assert!((3..=7).contains(&v));
        }
        let f = a.flag("x", 10);
        assert!(f.starts_with("x{") && f.ends_with('}') && f.len() == 13);
        let mut v: Vec<u32> = (0..20).collect();
        a.shuffle(&mut v);
        let mut sorted = v.clone();
        sorted.sort();
        assert_eq!(sorted, (0..20).collect::<Vec<_>>());
        assert_ne!(Rng::new([0; 4]).next_u64(), 0);
    }
}
