use lyracore_shared::form::PowerOnEntry;
use lyracore_shared::spell::taxonomy::{A_FORM_POWER_RETAINED, A_SHAPESHIFT};
use spacetimedb::{log, table, ReducerContext, Table};

use crate::spell::{auras_on, shed_auras, ShapeshiftState};
use crate::{game_spell, game_world_entity, Aura, WorldEntity};

/// A Form composed from the Module's building blocks. See docs/schema.md "Forms".
#[table(accessor = game_form, public)]
#[derive(Clone)]
pub struct Form {
    #[primary_key]
    pub form_id: u32,
    pub name: String,
    /// The spell whose Aura the Module applies when it puts a unit in this Form itself.
    pub spell_id: u32,
    pub acts_as_shapeshifted: bool,
    /// `lyracore_shared::packing::power_type`, the pool the power rule applies to.
    pub power_type: u8,
    /// A `lyracore_shared::form::PowerOnEntry` code.
    pub power_on_entry: u8,
    /// The class a unit with no Form enters this Form at world entry; zero is none.
    pub default_for_class: u8,
    /// The 1.12 client's form byte, `UNIT_FIELD_BYTES_1` byte 2.
    pub client_form: u8,
}

/// A passive spell a unit holds while in a Form.
#[table(
    accessor = game_form_spell,
    public,
    index(accessor = by_form, btree(columns = [form_id])),
    index(accessor = by_spell, btree(columns = [spell_id]))
)]
pub struct FormSpell {
    /// `lyracore_package_delta::packed_form_spell_id(form_id, spell_id)`.
    #[primary_key]
    pub id: u64,
    pub form_id: u32,
    pub spell_id: u32,
}

crate::outside_table_catalog!(game_form, game_form_spell);

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
            // Build-5875 SpellShapeshiftForm.dbc marks these forms as stances.
            acts_as_shapeshifted: !matches!(client_form, 17..=19 | 28 | 30 | 31),
        },
    })
}

/// Removes every shapeshift Aura but `kept`, with all effects of each removed spell.
pub(crate) fn keep_only(ctx: &ReducerContext, guid: u64, kept: u32) {
    let replaced: std::collections::BTreeSet<u32> = shapeshift_auras(ctx, guid)
        .map(|aura| aura.spell_id)
        .filter(|&spell| spell != kept)
        .collect();
    let removed = auras_on(ctx, guid)
        .filter(|aura| replaced.contains(&aura.spell_id))
        .collect();
    shed_auras(ctx, removed);
}

/// Re-derives the client form byte, the Form's passives and the equipment that depends on the
/// Form from the unit's shapeshift Aura. Entering a Form also applies its power rule. A client
/// byte no Form projects, such as a druid form `E_SET_STANCE` wrote, stays as it is.
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
        None if form_by_client_byte(ctx, current).is_some() => 0,
        None => current,
    };
    if current == client_form {
        return;
    }
    let left = current != 0;
    unit.unit_bytes_1 =
        (unit.unit_bytes_1 & !CLIENT_FORM_MASK) | (client_form << CLIENT_FORM_SHIFT);
    if let Some(form) = &form {
        unit.power = power_on_entry(ctx, &unit, form);
    }
    let unit = unit.update(ctx);
    if left {
        shed_shape_bound_auras(ctx, guid);
    }
    crate::combat::queued_strike::interrupt_form_bound(ctx, &unit);
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

/// vmangos `HandleShapeshiftBoosts` removes the unit's own Auras that need the Form it left.
fn shed_shape_bound_auras(ctx: &ReducerContext, guid: u64) {
    let spells = ctx.db.game_spell();
    let bound = auras_on(ctx, guid)
        .filter(|aura| aura.caster_guid == guid && aura.eff_kind != A_SHAPESHIFT)
        .filter(|aura| {
            spells
                .spell_id()
                .find(aura.spell_id)
                .is_some_and(|spell| spell.removed_on_shape_lost())
        })
        .collect();
    shed_auras(ctx, bound);
}

/// Applies the passives the unit's Form grants and removes every other Form's passives.
fn reconcile_passives(ctx: &ReducerContext, unit: &WorldEntity, form: Option<&Form>) {
    let desired: std::collections::BTreeSet<u32> = form
        .into_iter()
        .flat_map(|form| ctx.db.game_form_spell().by_form().filter(form.form_id))
        .map(|row| row.spell_id)
        .collect();
    let (stale, held): (Vec<Aura>, Vec<Aura>) = auras_on(ctx, unit.guid).partition(|aura| {
        !desired.contains(&aura.spell_id)
            && ctx
                .db
                .game_form_spell()
                .by_spell()
                .filter(aura.spell_id)
                .next()
                .is_some()
    });
    shed_auras(ctx, stale);
    for spell in desired {
        if !held.iter().any(|aura| aura.spell_id == spell) {
            crate::spell::apply_spell_auras(ctx, spell, unit.guid, unit.level as u8, 1);
        }
    }
}

/// Writes the client form byte of the Form a Character enters the world in onto its entity before
/// insertion, so the first CREATE peers receive already shows it.
pub(crate) fn stamp_entering_form(ctx: &ReducerContext, entity: &mut WorldEntity) {
    let form = active_form(ctx, entity.guid).or_else(|| default_form(ctx, entity.class()));
    if let Some(form) = form {
        entity.unit_bytes_1 = (entity.unit_bytes_1 & !CLIENT_FORM_MASK)
            | (u32::from(form.client_form) << CLIENT_FORM_SHIFT);
    }
}

/// A unit with no shapeshift Aura enters its class's default Form, as vmangos `Player::_LoadAuras`
/// casts Battle Stance on a warrior; then the projection catches up with the Aura set.
pub(crate) fn enter_world(ctx: &ReducerContext, guid: u64) {
    if let Some(unit) = ctx.db.game_world_entity().guid().find(guid) {
        if shapeshift_auras(ctx, guid).next().is_none() {
            if let Some(form) = default_form(ctx, unit.class()) {
                apply_form_spell(ctx, &unit, &form);
            }
        }
    }
    project_form(ctx, guid);
}

/// Publish repair: keeps one shapeshift Aura, and gives a unit with none the Form its client form
/// byte shows, else its class default.
pub(crate) fn repair(ctx: &ReducerContext, guid: u64) {
    let Some(unit) = ctx.db.game_world_entity().guid().find(guid) else {
        return;
    };
    if let Some(newest) = shapeshift_auras(ctx, guid).max_by_key(|aura| aura.id) {
        keep_only(ctx, guid, newest.spell_id);
    } else if let Some(form) =
        form_by_client_byte(ctx, unit.form()).or_else(|| default_form(ctx, unit.class()))
    {
        apply_form_spell(ctx, &unit, &form);
    }
    project_form(ctx, guid);
}

fn form_by_client_byte(ctx: &ReducerContext, client_form: u32) -> Option<Form> {
    (client_form != 0)
        .then(|| {
            ctx.db
                .game_form()
                .iter()
                .filter(|form| u32::from(form.client_form) == client_form)
                .min_by_key(|form| form.form_id)
        })
        .flatten()
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
