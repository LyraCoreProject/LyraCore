#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZeroMask {
    AdmitsAll,
    AdmitsNone,
}

// Race and class ids are one-based bit indices.
pub fn mask_admits(mask: u32, id: u8, zero: ZeroMask) -> bool {
    if mask == 0 {
        return zero == ZeroMask::AdmitsAll;
    }
    (1..=32).contains(&id) && mask & (1u32 << (id - 1)) != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mask_admits_the_ids_of_its_one_based_bits() {
        for zero in [ZeroMask::AdmitsAll, ZeroMask::AdmitsNone] {
            assert!(mask_admits(77, 1, zero));
            assert!(mask_admits(77, 3, zero));
            assert!(!mask_admits(77, 2, zero));
            assert!(!mask_admits(77, 5, zero));
            assert!(!mask_admits(77, 0, zero));
        }
    }

    #[test]
    fn a_zero_mask_admits_what_the_caller_says() {
        assert!(mask_admits(0, 1, ZeroMask::AdmitsAll));
        assert!(mask_admits(0, 11, ZeroMask::AdmitsAll));
        assert!(!mask_admits(0, 8, ZeroMask::AdmitsNone));
    }

    #[test]
    fn the_mask_range_ends_at_bit_31() {
        let bit31: u32 = 1 << 31;
        assert!(mask_admits(bit31, 32, ZeroMask::AdmitsNone));
        assert!(!mask_admits(bit31, 33, ZeroMask::AdmitsNone));
        assert!(!mask_admits(u32::MAX, u8::MAX, ZeroMask::AdmitsNone));
    }
}
