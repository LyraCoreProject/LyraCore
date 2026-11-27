mod depth;
mod impact;
pub(crate) mod resolve;
mod targeting;

pub(crate) use impact::land_spell_impact;
pub(crate) use resolve::{
    apply_item_auras, apply_spell_auras, begin_cast, begin_loaded_cast, cast_aura_triggered,
    cast_triggered, charge_spell_cost, check_cast_start_gates, clear_dead_callback_cast_admission,
    complete_cast, record_spell_cooldown, resolve_cast, start_creature_spell, start_spell_cooldown,
    CastRequest, CreatureSpellCasterAdmission, CreatureSpellStart, CreatureSpellStartMode,
    CreatureSpellTarget, LoadedSpell,
};
#[cfg(feature = "debug_reducers")]
pub(crate) use targeting::blink_forward;
pub(crate) use targeting::{
    apply_direct_heal, apply_effect, aura_apply, aura_has_visible_slot, end_ground_area,
    faction_target_violation, helps_units, holds_channelled_area, is_channelled_area,
    place_ground_area, queued_strike_chain, recipe_reagents, revive_amount, select_targets,
    spell_misses, ItemGrantOutcome,
};
