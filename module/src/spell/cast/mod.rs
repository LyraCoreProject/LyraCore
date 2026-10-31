pub(crate) mod resolve;
mod targeting;

#[cfg(test)]
pub(crate) use resolve::passive_applies_effect_kind;
#[cfg(feature = "debug_reducers")]
pub(crate) use resolve::resolve_cast;
pub(crate) use resolve::{
    apply_spell_auras, begin_cast, begin_cast_with_admission, begin_item_cast, cast_triggered,
    charge_spell_cost, check_cast_start_gates, check_pending_cast_los,
    clear_dead_callback_cast_admission, resolve_cast_at, resolve_cast_at_typed,
    start_creature_spell, start_spell_cooldown, CreatureSpellCasterAdmission, CreatureSpellStart,
    CreatureSpellStartMode, CreatureSpellTarget,
};
#[cfg(feature = "debug_reducers")]
pub(crate) use targeting::blink_forward;
pub(crate) use targeting::{
    apply_direct_heal, apply_effect, aura_apply, dynamic_object_guid, faction_target_violation,
    recipe_reagents, revive_amount, select_targets, ItemGrantOutcome,
};
