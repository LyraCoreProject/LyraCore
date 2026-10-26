// Prices are in copper.
pub const BANK_SLOT_PRICES: [u32; 6] = [1_000, 10_000, 100_000, 250_000, 250_000, 250_000];

pub const MAX_BANK_BAG_SLOTS: u8 = BANK_SLOT_PRICES.len() as u8;

pub fn next_bank_slot_price(owned: u8) -> Option<u32> {
    BANK_SLOT_PRICES.get(owned as usize).copied()
}

// SMSG_BUY_BANK_SLOT_RESULT wire values.
pub mod result {
    pub const FAILED_TOO_MANY: u8 = 0;
    pub const INSUFFICIENT_FUNDS: u8 = 1;
    pub const NOT_BANKER: u8 = 2;
    pub const OK: u8 = 3;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ladder_charges_the_six_vanilla_rungs_in_order() {
        let rungs: Vec<u32> = (0..MAX_BANK_BAG_SLOTS)
            .map(|owned| next_bank_slot_price(owned).expect("a slot is still for sale"))
            .collect();
        assert_eq!(
            rungs,
            vec![1_000, 10_000, 100_000, 250_000, 250_000, 250_000]
        );
    }

    #[test]
    fn there_is_no_seventh_rung() {
        assert_eq!(next_bank_slot_price(MAX_BANK_BAG_SLOTS), None);
        assert_eq!(next_bank_slot_price(u8::MAX), None);
    }
}
