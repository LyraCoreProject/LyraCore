use lyracore_shared::form::PowerOnEntry;
use lyracore_shared::spell::taxonomy::{A_FORM_POWER_RETAINED, A_SHAPESHIFT};
use spacetimedb::{log, ReducerContext, Table};

use crate::spell::tables::game_spell_form;
use crate::spell::{auras_on, shed_auras, FormRequirement, ShapeshiftState};
use crate::world::power_pool::is_power_type;
use crate::{
    game_player_spell, game_spell, game_spell_effect, game_world_entity, Aura, WorldEntity,
};

pub(crate) mod tables;

pub(crate) use tables::{game_form, game_form_spell, Form};

const CLIENT_FORM_SHIFT: u32 = 16;
const CLIENT_FORM_MASK: u32 = 0xFF << CLIENT_FORM_SHIFT;

fn shapeshift_auras(ctx: &ReducerContext, guid: u64) -> impl Iterator<Item = Aura> {
    auras_on(ctx, guid).filter(|aura| aura.eff_kind == A_SHAPESHIFT)
}

/// The Form the unit's one shapeshift Aura names.
pub(crate) fn active_form(ctx: &ReducerContext, guid: u64) -> Option<Form> {
    let aura = shapeshift_auras(ctx, guid).max_by_key(|aura| aura.id)?;
    let form = ctx.db.game_form().form_id().find(aura.eff_p0.max(0) as u32);
    if form.is_none() {
        log::warn!(
            "unit {guid} holds shapeshift Aura {} for Form {} with no game_form row",
            aura.spell_id,
            aura.eff_p0
        );
    }
    form
}

/// The unit's Form for the cast and equip Gates. A unit without a shapeshift Aura can still hold
/// a druid form that `E_SET_STANCE` wrote straight to the client form byte.
pub(crate) fn shapeshift_state(
    ctx: &ReducerContext,
    unit: &WorldEntity,
) -> Option<ShapeshiftState> {
    if let Some(form) = active_form(ctx, unit.guid) {
        return Some(ShapeshiftState {
            form_id: form.form_id,
            acts_as_shapeshifted: form.acts_as_shapeshifted,
        });
    }
    let client_form = unit.form();
    if client_form == 0 {
        return None;
    }
    Some(match form_by_client_byte(ctx, client_form) {
        Some(form) => ShapeshiftState {
            form_id: form.form_id,
            acts_as_shapeshifted: form.acts_as_shapeshifted,
        },
        None => ShapeshiftState {
            form_id: client_form,
            // Build-5875 SpellShapeshiftForm.dbc marks these forms as stances. Druid forms, which
            // have no Form rows yet, reach this, and so do warrior bytes 17-19 before the spell
            // reimport writes Forms; #1046 PR 4 removes the table.
            acts_as_shapeshifted: !matches!(client_form, 17..=19 | 28 | 30 | 31),
        },
    })
}

/// A shapeshift Aura of `spell_id` naming `form_id` was applied: the unit leaves every other
/// Form, the Form's pool is shown and the power rule runs. The caller's Aura refresh projects the
/// Form once.
pub(crate) fn entered(ctx: &ReducerContext, guid: u64, spell_id: u32, form_id: u32) {
    let replaced: std::collections::BTreeSet<u32> = shapeshift_auras(ctx, guid)
        .map(|aura| aura.spell_id)
        .filter(|&spell| spell != spell_id)
        .collect();
    let removed = auras_on(ctx, guid)
        .filter(|aura| replaced.contains(&aura.spell_id))
        .collect();
    crate::spell::shed_auras_before_projection(ctx, removed);
    if let (Some(mut unit), Some(form)) = (
        ctx.db.game_world_entity().guid().find(guid),
        ctx.db.game_form().form_id().find(form_id),
    ) {
        let shown = (unit.unit_bytes_0, unit.power);
        show_form_pool(&mut unit, Some(&form));
        unit.power = power_on_entry(ctx, &unit, &form);
        if (unit.unit_bytes_0, unit.power) != shown {
            unit.update(ctx);
        }
    }
}

/// Shows the pool the unit's Form names, or the unit's own pool when it has no Form.
fn show_form_pool(unit: &mut WorldEntity, form: Option<&Form>) {
    let power_type = form.map_or(unit.own_pool().power_type, |form| form.power_type);
    if is_power_type(power_type) {
        unit.show_pool(power_type);
    } else if let Some(form) = form {
        log::warn!(
            "Form {} names unknown power type {power_type}",
            form.form_id
        );
    }
}

/// A shapeshift Aura was refreshed: as vmangos removes and reapplies the modifier, the unit's
/// Form-bound Auras end before the Form is entered again.
pub(crate) fn refreshed(ctx: &ReducerContext, guid: u64, spell_id: u32, form_id: u32) {
    end_form_bound(ctx, guid);
    entered(ctx, guid, spell_id, form_id);
}

/// A shapeshift Aura ended. A unit left in no Form gets the shapeshift effect's trigger spell.
pub(crate) fn exited(ctx: &ReducerContext, aura: &Aura) {
    let guid = aura.target_guid;
    end_form_bound(ctx, guid);
    let next = ctx
        .db
        .game_spell_effect()
        .id()
        .find(aura.effect_id)
        .map_or(0, |effect| effect.trigger_spell);
    if next != 0 && shapeshift_auras(ctx, guid).next().is_none() {
        if let Some(unit) = ctx.db.game_world_entity().guid().find(guid) {
            crate::spell::apply_spell_auras(ctx, next, guid, unit.level as u8, 1);
        }
    }
}

/// As vmangos `HandleShapeshiftBoosts(false)`, the unit's own Auras that need a Form end, and so
/// does a Form-bound queued strike. Equipment Auras stay with the equipment reconcile.
fn end_form_bound(ctx: &ReducerContext, guid: u64) {
    let spells = ctx.db.game_spell();
    let bound = auras_on(ctx, guid)
        .filter(|held| held.caster_guid == guid && held.eff_kind != A_SHAPESHIFT)
        .filter(|held| held.item_guid(ctx) == 0)
        .filter(|held| {
            spells.spell_id().find(held.spell_id).is_some_and(|spell| {
                spell.removed_on_shape_lost(FormRequirement::of(ctx, spell.spell_id, None))
            })
        })
        .collect();
    shed_auras(ctx, bound);
    if let Some(unit) = ctx.db.game_world_entity().guid().find(guid) {
        crate::combat::queued_strike::interrupt_form_bound(ctx, &unit);
    }
}

/// Whether the unit holds an Aura whose spell locks it in its Form, as Nefarian's Berserk does.
pub(crate) fn locked(ctx: &ReducerContext, guid: u64) -> bool {
    let spells = ctx.db.game_spell();
    auras_on(ctx, guid).any(|aura| {
        spells.spell_id().find(aura.spell_id).is_some_and(|spell| {
            spell.cast_flags & lyracore_shared::spell::taxonomy::CAST_FLAG_LOCKS_FORM != 0
        })
    })
}

/// Re-derives from the unit's shapeshift Aura the client form byte, the shown pool, the passives
/// the Form brings and the equipment that depends on the Form. A byte no Form projects, or the
/// byte `E_SET_STANCE` wrote for the unit's `stance`, stays as it is.
pub(crate) fn project_form(ctx: &ReducerContext, guid: u64) {
    let Some(unit) = ctx.db.game_world_entity().guid().find(guid) else {
        return;
    };
    let form = active_form(ctx, guid);
    reconcile_passives(ctx, &unit, form.as_ref());
    let Some(mut unit) = ctx.db.game_world_entity().guid().find(guid) else {
        return;
    };
    let current = unit.form();
    let client_form = match &form {
        Some(form) => u32::from(form.client_form),
        None if unit.stance != 0
            && current == crate::spell::client_form_for_stance(unit.stance) =>
        {
            current
        }
        None if ctx
            .db
            .game_form()
            .iter()
            .any(|form| u32::from(form.client_form) == current) =>
        {
            0
        }
        None => current,
    };
    let shown = (unit.unit_bytes_1, unit.unit_bytes_0);
    unit.unit_bytes_1 =
        (unit.unit_bytes_1 & !CLIENT_FORM_MASK) | (client_form << CLIENT_FORM_SHIFT);
    show_form_pool(&mut unit, form.as_ref());
    if (unit.unit_bytes_1, unit.unit_bytes_0) != shown {
        unit = unit.update(ctx);
    }
    if unit.is_player() {
        crate::items::equip::reconcile(ctx, guid);
    }
    crate::spell::recompute_sheet(ctx, guid);
}

fn power_on_entry(ctx: &ReducerContext, unit: &WorldEntity, form: &Form) -> u32 {
    if unit.power_type() != form.power_type {
        return unit.power;
    }
    match PowerOnEntry::try_from(form.power_on_entry) {
        Ok(PowerOnEntry::Keep) => unit.power,
        Ok(PowerOnEntry::Zero) => 0,
        Ok(PowerOnEntry::KeepUpToRetained) => unit.power.min(retained_power(ctx, unit)),
        Err(code) => {
            log::warn!("Form {} has unknown power rule {code}", form.form_id);
            unit.power
        }
    }
}

fn retained_power(ctx: &ReducerContext, unit: &WorldEntity) -> u32 {
    auras_on(ctx, unit.guid)
        .filter(|aura| {
            aura.eff_kind == A_FORM_POWER_RETAINED && aura.eff_p0 == i32::from(unit.power_type())
        })
        .map(|aura| aura.amount.max(0) as u32)
        .max()
        .unwrap_or(0)
}

/// Whether the Form projection owns a known passive: its Form Set requires a Form with a
/// `game_form` row. Any other passive keeps the Aura its learning applied.
pub(crate) fn binds_to_forms(ctx: &ReducerContext, spell: &crate::Spell) -> bool {
    spell.binds_to_forms(FormRequirement::of(ctx, spell.spell_id, None))
        && ctx
            .db
            .game_spell_form()
            .by_spell()
            .filter(spell.spell_id)
            .any(|row| !row.excluded && ctx.db.game_form().form_id().find(row.form_id).is_some())
}

/// The passives the unit's Form brings: the Form's own, and every known passive whose Form Set
/// requires the Form (vmangos `HandleShapeshiftBoosts(true)`). Every other such passive goes,
/// except a learned spell that some Form names as its passive: the learning owns that Aura.
fn reconcile_passives(ctx: &ReducerContext, unit: &WorldEntity, form: Option<&Form>) {
    let form_id = form.map(|form| form.form_id);
    let spells = ctx.db.game_spell();
    let mut desired: std::collections::BTreeSet<u32> = form
        .into_iter()
        .flat_map(|form| ctx.db.game_form_spell().by_form().filter(form.form_id))
        .map(|row| row.spell_id)
        .collect();
    if form_id.is_some() {
        desired.extend(
            ctx.db
                .game_player_spell()
                .by_character()
                .filter(unit.guid)
                .map(|known| known.spell_id)
                .filter(|&spell_id| {
                    spells.spell_id().find(spell_id).is_some_and(|spell| {
                        spell.applies_on_form_entry(FormRequirement::of(ctx, spell_id, form_id))
                    })
                }),
        );
    }
    let known: std::collections::BTreeSet<u32> = ctx
        .db
        .game_player_spell()
        .by_character()
        .filter(unit.guid)
        .map(|known| known.spell_id)
        .collect();
    let (stale, held): (Vec<Aura>, Vec<Aura>) = auras_on(ctx, unit.guid).partition(|aura| {
        !desired.contains(&aura.spell_id)
            && aura.caster_guid == unit.guid
            && aura.item_guid(ctx) == 0
            && ((!known.contains(&aura.spell_id)
                && ctx
                    .db
                    .game_form_spell()
                    .by_spell()
                    .filter(aura.spell_id)
                    .next()
                    .is_some())
                || spells
                    .spell_id()
                    .find(aura.spell_id)
                    .is_some_and(|spell| binds_to_forms(ctx, &spell)))
    });
    shed_auras(ctx, stale);
    for spell in desired {
        if !held.iter().any(|aura| aura.spell_id == spell) {
            crate::spell::apply_spell_auras(ctx, spell, unit.guid, unit.level as u8, 1);
        }
    }
}

/// Writes onto a Character's entity, before insertion, the client form byte of the Form it enters
/// the world in, so the first CREATE peers receive already shows it.
pub(crate) fn stamp_entering_form(ctx: &ReducerContext, entity: &mut WorldEntity) {
    let client_form =
        match active_form(ctx, entity.guid).or_else(|| default_form(ctx, entity.class())) {
            Some(form) => u32::from(form.client_form),
            // A Shard without Forms (docs/danger-zones.md "Warrior stance migration") keeps the
            // Battle Stance byte every rage class entered with before Forms.
            None if ctx.db.game_form().iter().next().is_none()
                && entity.power_type() == lyracore_shared::packing::power_type::RAGE =>
            {
                17
            }
            None => return,
        };
    entity.unit_bytes_1 =
        (entity.unit_bytes_1 & !CLIENT_FORM_MASK) | (client_form << CLIENT_FORM_SHIFT);
}

/// A Character body entering the world, by login or otherwise, with no shapeshift Aura enters its
/// class's default Form, as vmangos `Player::_LoadAuras` casts Battle Stance on every loaded
/// warrior; then the projection catches up with the Aura set.
pub(crate) fn enter_world(ctx: &ReducerContext, guid: u64) {
    let Some(unit) = ctx.db.game_world_entity().guid().find(guid) else {
        return;
    };
    let holds_form = shapeshift_auras(ctx, guid).next().is_some();
    let entered_default = !holds_form
        && default_form(ctx, unit.class()).is_some_and(|form| {
            apply_form_spell(ctx, &unit, &form);
            shapeshift_auras(ctx, guid).next().is_some()
        });
    // Entering the default Form projects through its Aura.
    if !entered_default {
        project_form(ctx, guid);
    }
}

/// Publish repair: keeps one shapeshift Aura, and gives a unit with none the Form its client form
/// byte shows, else its class default.
pub(crate) fn repair(ctx: &ReducerContext, guid: u64) {
    let Some(unit) = ctx.db.game_world_entity().guid().find(guid) else {
        return;
    };
    if let Some(newest) = shapeshift_auras(ctx, guid).max_by_key(|aura| aura.id) {
        let others = auras_on(ctx, guid)
            .filter(|aura| aura.eff_kind == A_SHAPESHIFT && aura.spell_id != newest.spell_id)
            .map(|aura| aura.spell_id)
            .collect::<std::collections::BTreeSet<_>>();
        let removed = auras_on(ctx, guid)
            .filter(|aura| others.contains(&aura.spell_id))
            .collect();
        shed_auras(ctx, removed);
    } else if let Some(form) =
        form_by_client_byte(ctx, unit.form()).or_else(|| default_form(ctx, unit.class()))
    {
        apply_form_spell(ctx, &unit, &form);
    }
    project_form(ctx, guid);
}

/// The Form whose id is the client byte and which projects that byte; a Form's fallback byte never
/// maps back to that Form.
fn form_by_client_byte(ctx: &ReducerContext, client_form: u32) -> Option<Form> {
    ctx.db
        .game_form()
        .form_id()
        .find(client_form)
        .filter(|form| u32::from(form.client_form) == client_form)
}

fn default_form(ctx: &ReducerContext, class: u8) -> Option<Form> {
    if class == 0 {
        return None;
    }
    ctx.db
        .game_form()
        .iter()
        .filter(|form| form.default_for_class == class)
        .min_by_key(|form| form.form_id)
}

fn apply_form_spell(ctx: &ReducerContext, unit: &WorldEntity, form: &Form) {
    crate::spell::apply_spell_auras(ctx, form.spell_id, unit.guid, unit.level as u8, 1);
}
