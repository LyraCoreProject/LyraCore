use lyracore_shared::spell::taxonomy::{
    A_MOD_DAMAGE_TAKEN, STANCE_BEAR, STANCE_CAT, STANCE_DEFENSIVE, STANCE_DIRE_BEAR,
};
use spacetimedb::ReducerContext;

use super::taxonomy::{
    STANCE_DEFENSIVE_DMG_DONE_PCT, STANCE_DEFENSIVE_DR_PCT, STANCE_DEFENSIVE_THREAT_PCT,
};
use crate::game_world_entity;

pub(crate) fn stance_allows(mask: u8, stance: u8) -> bool {
    mask == 0 || (stance < 8 && (mask & (1u8 << stance)) != 0)
}

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

pub(crate) fn stance_damage_taken_pct(stance: u8) -> i32 {
    if stance == STANCE_DEFENSIVE {
        STANCE_DEFENSIVE_DR_PCT
    } else {
        0
    }
}

pub(crate) fn stance_damage_done_pct(stance: u8) -> i32 {
    if stance == STANCE_DEFENSIVE {
        STANCE_DEFENSIVE_DMG_DONE_PCT
    } else {
        0
    }
}

pub(crate) fn stance_threat_pct(stance: u8) -> i32 {
    if stance == STANCE_DEFENSIVE {
        STANCE_DEFENSIVE_THREAT_PCT
    } else {
        0
    }
}

pub(crate) fn damage_taken_bonus(ctx: &ReducerContext, target_guid: u64) -> i32 {
    let aura_pct =
        crate::modifiers::aura_total(ctx, target_guid, |a| a.eff_kind == A_MOD_DAMAGE_TAKEN);
    let stance_pct = ctx
        .db
        .game_world_entity()
        .guid()
        .find(target_guid)
        .map(|t| stance_damage_taken_pct(t.stance))
        .unwrap_or(0);
    aura_pct + stance_pct
}
