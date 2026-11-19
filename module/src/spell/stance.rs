use lyracore_shared::spell::taxonomy::{STANCE_BEAR, STANCE_CAT, STANCE_DIRE_BEAR};

// Client build 5875 expects a ShapeshiftForm id in UNIT_FIELD_BYTES_1 byte 2.
pub(crate) fn client_form_for_stance(stance: u8) -> u32 {
    match stance {
        STANCE_BEAR => 5,
        STANCE_CAT => 1,
        STANCE_DIRE_BEAR => 8,
        _ => stance as u32 + 17,
    }
}

pub(crate) fn stance_switch_clears_power(old_stance: u8, new_stance: u8, power_type: u8) -> bool {
    old_stance != new_stance && power_type == lyracore_shared::packing::power_type::RAGE
}

pub(crate) fn form_recast_toggles_off(current: u8, recast: u8) -> bool {
    current == recast && recast >= STANCE_BEAR
}
