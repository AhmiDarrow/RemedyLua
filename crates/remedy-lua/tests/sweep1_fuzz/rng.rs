//! SplitMix64: small, seedable, good enough for fuzzing.

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// 0..n (n > 0).
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    /// lo..=hi.
    pub fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next() % ((hi - lo + 1) as u64)) as i64
    }
    /// True with probability pct/100.
    pub fn pct(&mut self, pct: u32) -> bool {
        (self.next() % 100) < u64::from(pct)
    }
    pub fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
    /// An index chosen by weight.
    pub fn weighted(&mut self, ws: &[u32]) -> usize {
        let total: u32 = ws.iter().sum();
        let mut x = (self.next() % u64::from(total.max(1))) as u32;
        for (i, &w) in ws.iter().enumerate() {
            if x < w {
                return i;
            }
            x -= w;
        }
        ws.len() - 1
    }
}
