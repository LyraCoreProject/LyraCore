//! Bank bag slot purchase vocabulary: the vanilla price ladder and the wire result codes.
//!
//! The prices are the well-known vanilla copper values, typed by hand. No DBC is read, copied, or
//! shipped — the licensing firewall stands.

/// Copper charged for each bank bag slot, cheapest (first bought) first: 10s, 1g, 10g, 25g, 25g, 25g.
pub const BANK_SLOT_PRICES: [u32; 6] = [1_000, 10_000, 100_000, 250_000, 250_000, 250_000];

/// How many bank bag slots a character can ever own.
pub const MAX_BANK_BAG_SLOTS: u8 = BANK_SLOT_PRICES.len() as u8;

/// Copper for the NEXT slot given how many are already owned. `None` once all six are owned, which is
/// the "no more to buy" refusal. Pure — unit-tested.
pub fn next_bank_slot_price(owned: u8) -> Option<u32> {
    BANK_SLOT_PRICES.get(owned as usize).copied()
}

/// `SMSG_BUY_BANK_SLOT_RESULT` values, in the client's own numbering. The module tags a refused
/// purchase with `[N]` from this set so the gateway maps the outcome by code instead of matching error
/// prose (the trainer `[N]` precedent). These ARE the wire values — relay them, don't remap them.
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
