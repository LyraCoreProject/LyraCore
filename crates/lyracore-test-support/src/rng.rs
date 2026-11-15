pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        // xorshift64* degenerates at zero.
        Self(seed | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % (n.max(1)) as u64) as usize
    }

    pub fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| (self.next_u64() >> 24) as u8).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::Rng;

    #[test]
    fn the_seeded_generator_is_reproducible_and_seed_dependent() {
        let a: Vec<u8> = Rng::new(7).bytes(64);
        let b: Vec<u8> = Rng::new(7).bytes(64);
        assert_eq!(a, b, "the same seed must replay the same byte stream");

        let c: Vec<u8> = Rng::new(8).bytes(64);
        assert_ne!(a, c, "different seeds must explore different inputs");

        assert!(
            a.windows(2).any(|w| w[0] != w[1]),
            "the generator emitted a constant stream: {a:02X?}"
        );
    }
}
