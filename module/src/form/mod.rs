use lyracore_shared::form::{PowerOnEntry, OWN_POOL};
use lyracore_shared::spell::taxonomy::{
    A_CONTROL, A_FORM_ENTRY_CHANCE, A_FORM_POWER_RETAINED, A_MOD_SPEED, A_SHAPESHIFT, M_ROOT,
    SPEED_MOVE,
};
use spacetimedb::{log, ReducerContext, Table};

use crate::spell::tables::game_spell_form;
use crate::spell::{auras_on, shed_auras, FormRequirement, ShapeshiftState};
use crate::world::power_pool::is_power_type;
use crate::{
    game_player_spell, game_spell, game_spell_chain, game_spell_effect, game_talent,
    game_world_entity, Aura, WorldEntity,
};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) mod tables;

pub(crate) use tables::{game_form, game_form_spell, Form, FormSpell};

const CLIENT_FORM_SHIFT: u32 = 16;
const CLIENT_FORM_MASK: u32 = 0xFF << CLIENT_FORM_SHIFT;

fn shapeshift_auras(ctx: &ReducerContext, guid: u64) -> impl Iterator<Item = Aura> {
    auras_on(ctx, guid).filter(|aura| aura.eff_kind == A_SHAPESHIFT)
}

/// The Form the unit's one shapeshift Aura names.
pub(crate) fn active_form(ctx: &ReducerContext, guid: u64) -> Option<Form> {
    let aura = shapeshift_auras(ctx, guid).max_by_key(|aura| aura.id)?;
    let form = ctx.db.game_form().form_id().find(aura.eff_p0.max(0) as u32);
    if form.is_none() && first_warning_for(aura.id) {
        log::warn!(
            "unit {guid} holds shapeshift Aura {} for Form {} with no game_form row",
            aura.spell_id,
            aura.eff_p0
        );
    }
    form
}

thread_local! {
    static WARNED_AURAS: std::cell::RefCell<BTreeSet<u64>> =
        const { std::cell::RefCell::new(BTreeSet::new()) };
}

/// Records a warning for the Aura row `aura_id`; true only the first time in this Module instance.
fn first_warning_for(aura_id: u64) -> bool {
    WARNED_AURAS.with(|warned| warned.borrow_mut().insert(aura_id))
}

/// The unit's Form for the cast and equip Gates.
pub(crate) fn shapeshift_state(
    ctx: &ReducerContext,
    unit: &WorldEntity,
) -> Option<ShapeshiftState> {
    active_form(ctx, unit.guid).map(|form| ShapeshiftState {
        form_id: form.form_id,
        acts_as_shapeshifted: form.acts_as_shapeshifted,
    })
}

/// How a unit fights in its Form. See docs/schema.md "Forms".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CombatProfile {
    #[default]
    Weapon,
    /// Weapons are set aside: both hands swing level-based natural attacks every `attack_ms`.
    Natural {
        attack_ms: u32,
        attack_power_from_agility: bool,
    },
}

impl CombatProfile {
    pub(crate) fn is_natural(self) -> bool {
        matches!(self, Self::Natural { .. })
    }
}

impl Form {
    /// The power type of the pool the Form shows on `unit`.
    fn shown_power_type(&self, unit: &WorldEntity) -> u8 {
        match self.power_type {
            OWN_POOL => unit.own_pool().power_type,
            power_type => power_type,
        }
    }

    fn combat_profile(&self) -> CombatProfile {
        match self.natural_attack_ms {
            0 => CombatProfile::Weapon,
            attack_ms => CombatProfile::Natural {
                attack_ms,
                attack_power_from_agility: self.attack_power_from_agility,
            },
        }
    }
}

pub(crate) fn combat_profile(ctx: &ReducerContext, guid: u64) -> CombatProfile {
    active_form(ctx, guid).map_or(CombatProfile::Weapon, |form| form.combat_profile())
}

/// Whether the unit's Form keeps it from mounting and from taking a taxi, as every Form that acts
/// as shapeshifted does.
pub(crate) fn blocks_mounting(ctx: &ReducerContext, guid: u64) -> bool {
    active_form(ctx, guid).is_some_and(|form| form.acts_as_shapeshifted || form.blocks_mounting)
}

/// A shapeshift Aura of `spell_id` naming `form_id` was applied: the unit leaves every other
/// Form, the Form's pool is shown and the power rule runs. The caller's Aura refresh projects the
/// Form once.
pub(crate) fn entered(ctx: &ReducerContext, guid: u64, spell_id: u32, form_id: u32) {
    let replaced: BTreeSet<u32> = shapeshift_auras(ctx, guid)
        .map(|aura| aura.spell_id)
        .filter(|&spell| spell != spell_id)
        .collect();
    let removed = auras_on(ctx, guid)
        .filter(|aura| replaced.contains(&aura.spell_id))
        .collect();
    crate::spell::shed_auras_before_projection(ctx, removed);
    let Some(form) = ctx.db.game_form().form_id().find(form_id) else {
        return;
    };
    if form.clears_snares {
        clear_snares(ctx, guid);
    }
    if let Some(mut unit) = ctx.db.game_world_entity().guid().find(guid) {
        let shown = (unit.unit_bytes_0, unit.power);
        show_form_pool(&mut unit, Some(&form));
        unit.power = power_on_entry(ctx, &unit, &form);
        if (unit.unit_bytes_0, unit.power) != shown {
            unit = unit.update(ctx);
        }
        cast_entry_chance_spell(ctx, &unit, &form);
    }
}

/// vmangos Furor: entering the Form casts its entry spell at the chance of the unit's
/// `A_FORM_ENTRY_CHANCE` Aura whose spell's Form Set requires this Form.
fn cast_entry_chance_spell(ctx: &ReducerContext, unit: &WorldEntity, form: &Form) {
    if form.entry_chance_spell == 0 {
        return;
    }
    let chance = auras_on(ctx, unit.guid)
        .filter(|aura| {
            aura.eff_kind == A_FORM_ENTRY_CHANCE
                && FormRequirement::of(ctx, aura.spell_id, Some(form.form_id)).requires_this
        })
        .map(|aura| aura.amount.max(0) as u32)
        .max()
        .unwrap_or(0);
    if ctx.random::<u32>() % 100 >= chance {
        return;
    }
    let spell = form.entry_chance_spell;
    if let Err(error) =
        crate::spell::cast_triggered(ctx, unit.guid, spell, unit.level as u8, unit.guid)
    {
        log::info!("Form {} entry spell {spell} failed: {error}", form.form_id);
    }
}

// vmangos SpellDefines.h MECHANIC_NOT_REMOVED_BY_SHAPESHIFT: charm, disoriented, fear, pacify, stun,
// freeze, banish, shackle, turn, horror, daze and sapped.
const MECHANICS_A_SHAPESHIFT_KEEPS: [u8; 12] = [1, 2, 5, 8, 12, 13, 18, 20, 23, 24, 27, 30];

/// vmangos Shapeshift Form Effect 9033: every root with a mechanic ends, and so does every
/// movement slow with a mechanic that a shapeshift does not keep.
fn clear_snares(ctx: &ReducerContext, guid: u64) {
    let freed: BTreeSet<u32> = auras_on(ctx, guid)
        .filter(|aura| {
            let root = aura.eff_kind == A_CONTROL && aura.eff_p0 == M_ROOT;
            let slow = aura.eff_kind == A_MOD_SPEED
                && aura.eff_p0 == i32::from(SPEED_MOVE)
                && aura.amount < 0;
            if !root && !slow {
                return false;
            }
            let mechanics = spell_mechanics(ctx, aura.spell_id);
            mechanics != 0 && (root || mechanics & mechanic_mask(MECHANICS_A_SHAPESHIFT_KEEPS) == 0)
        })
        .map(|aura| aura.spell_id)
        .collect();
    if freed.is_empty() {
        return;
    }
    let ended = auras_on(ctx, guid)
        .filter(|aura| freed.contains(&aura.spell_id))
        .collect();
    shed_auras(ctx, ended);
}

/// vmangos `SpellEntry::GetAllSpellMechanicMask`: the spell's mechanic and each effect's.
fn spell_mechanics(ctx: &ReducerContext, spell_id: u32) -> u32 {
    let header = ctx
        .db
        .game_spell()
        .spell_id()
        .find(spell_id)
        .map_or(0, |spell| spell.mechanic);
    mechanic_mask(
        std::iter::once(header).chain(
            ctx.db
                .game_spell_effect()
                .by_spell()
                .filter(spell_id)
                .map(|effect| effect.effect_mechanic),
        ),
    )
}

/// Bit `mechanic - 1` for each Spell.dbc mechanic; zero is none.
fn mechanic_mask(mechanics: impl IntoIterator<Item = u8>) -> u32 {
    mechanics
        .into_iter()
        .filter(|mechanic| (1..=32).contains(mechanic))
        .fold(0, |mask, mechanic| mask | 1 << (mechanic - 1))
}

/// Whether a Form's `power_type` names a pool the Module keeps or the unit's own pool.
pub(crate) fn is_form_power_type(power_type: u8) -> bool {
    power_type == OWN_POOL || is_power_type(power_type)
}

/// Shows the pool the unit's Form names, or the unit's own pool when it has no Form.
fn show_form_pool(unit: &mut WorldEntity, form: Option<&Form>) {
    let power_type = form.map_or(unit.own_pool().power_type, |form| {
        form.shown_power_type(unit)
    });
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
    if ctx
        .db
        .game_form()
        .form_id()
        .find(aura.eff_p0.max(0) as u32)
        .is_some_and(|form| form.clears_snares)
    {
        clear_snares(ctx, guid);
    }
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

/// Re-derives from the unit's shapeshift Aura the client form byte, the shown pool, the model, the
/// passives the Form brings and the equipment that depends on the Form. A byte no Form projects
/// stays as it is.
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
    crate::spell::recompute_display(ctx, guid);
    crate::spell::recompute_sheet(ctx, guid);
}

fn power_on_entry(ctx: &ReducerContext, unit: &WorldEntity, form: &Form) -> u32 {
    if unit.power_type() != form.shown_power_type(unit) {
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

/// The passives the unit's Form brings: the Form's own whose conditions the unit meets, and every
/// known passive whose Form Set requires the Form (vmangos `HandleShapeshiftBoosts(true)`). Every
/// other such passive goes, except a learned spell that some Form names as its passive: the
/// learning owns that Aura.
fn reconcile_passives(ctx: &ReducerContext, unit: &WorldEntity, form: Option<&Form>) {
    let form_id = form.map(|form| form.form_id);
    let spells = ctx.db.game_spell();
    let known: BTreeSet<u32> = ctx
        .db
        .game_player_spell()
        .by_character()
        .filter(unit.guid)
        .map(|known| known.spell_id)
        .collect();
    let mut desired: BTreeMap<u32, FirstEffectPoints> = form
        .into_iter()
        .flat_map(|form| ctx.db.game_form_spell().by_form().filter(form.form_id))
        .filter_map(|row| Some((row.spell_id, granted_points(ctx, unit.guid, &known, &row)?)))
        .collect();
    if form_id.is_some() {
        for &spell_id in &known {
            if spells.spell_id().find(spell_id).is_some_and(|spell| {
                spell.applies_on_form_entry(FormRequirement::of(ctx, spell_id, form_id))
            }) {
                desired.entry(spell_id).or_insert(None);
            }
        }
    }
    let (stale, held): (Vec<Aura>, Vec<Aura>) = auras_on(ctx, unit.guid).partition(|aura| {
        !desired.contains_key(&aura.spell_id)
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
    for (spell, points) in desired {
        if held.iter().any(|aura| aura.spell_id == spell) {
            continue;
        }
        match points {
            Some(points) => crate::spell::apply_spell_auras_with_points(
                ctx,
                spell,
                unit.guid,
                unit.level as u8,
                points,
            ),
            None => crate::spell::apply_spell_auras(ctx, spell, unit.guid, unit.level as u8, 1),
        }
    }
}

/// The amount a Form passive's first effect takes; `None` keeps the spell's own.
type FirstEffectPoints = Option<i32>;

/// The points a Form passive row grants the unit, or `None` when the unit does not meet its
/// conditions, as vmangos `HandleShapeshiftBoosts` casts Leader of the Pack and Heart of the Wild.
fn granted_points(
    ctx: &ReducerContext,
    guid: u64,
    known: &BTreeSet<u32>,
    row: &FormSpell,
) -> Option<FirstEffectPoints> {
    if row.required_spell != 0 && rank_line(ctx, row.required_spell).is_disjoint(known) {
        return None;
    }
    if row.amount_spell == 0 {
        return Some(None);
    }
    let line = rank_line(ctx, row.amount_spell);
    auras_on(ctx, guid)
        .filter(|aura| line.contains(&aura.spell_id))
        .min_by_key(|aura| aura.effect_id)
        .map(|aura| Some(aura.amount))
}

/// `spell_id` and the later ranks of the talent or spell chain it is the first rank of.
fn rank_line(ctx: &ReducerContext, spell_id: u32) -> BTreeSet<u32> {
    let mut line: BTreeSet<u32> = ctx
        .db
        .game_spell_chain()
        .by_first()
        .filter(spell_id)
        .map(|rank| rank.spell_id)
        .collect();
    line.insert(spell_id);
    if let Some(talent) = ctx
        .db
        .game_talent()
        .iter()
        .find(|talent| talent.spell_id == spell_id)
    {
        line.extend(
            [
                talent.rank_spell_2,
                talent.rank_spell_3,
                talent.rank_spell_4,
                talent.rank_spell_5,
            ]
            .into_iter()
            .filter(|&rank| rank != 0),
        );
    }
    line
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
/// byte shows, else its class default. A dead or ghost unit enters no Form its death would end.
pub(crate) fn repair(ctx: &ReducerContext, guid: u64) {
    let Some(unit) = ctx.db.game_world_entity().guid().find(guid) else {
        return;
    };
    if let Some(newest) = shapeshift_auras(ctx, guid).max_by_key(|aura| aura.id) {
        let others = auras_on(ctx, guid)
            .filter(|aura| aura.eff_kind == A_SHAPESHIFT && aura.spell_id != newest.spell_id)
            .map(|aura| aura.spell_id)
            .collect::<BTreeSet<_>>();
        let removed = auras_on(ctx, guid)
            .filter(|aura| others.contains(&aura.spell_id))
            .collect();
        shed_auras(ctx, removed);
    } else if let Some(form) = form_by_client_byte(ctx, unit.form())
        .or_else(|| default_form(ctx, unit.class()))
        .filter(|form| !unit.dead || survives_death(ctx, form))
    {
        apply_form_spell(ctx, &unit, &form);
    }
    project_form(ctx, guid);
}

fn survives_death(ctx: &ReducerContext, form: &Form) -> bool {
    ctx.db
        .game_spell()
        .spell_id()
        .find(form.spell_id)
        .is_some_and(|spell| crate::spell::aura_survives_death(&spell))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::power_pool::PowerPool;
    use lyracore_shared::packing::{power_type::*, unit_bytes_0};

    const DRUID: u8 = 11;
    const ROGUE: u8 = 4;

    fn unit(class: u8, power_type: u8, power: u32, max_power: u32) -> WorldEntity {
        let mut unit = crate::helpers::tests::entity(7, 0, 0, 0, 0);
        unit.unit_bytes_0 = unit_bytes_0(4, class, 0, power_type);
        unit.power = power;
        unit.max_power = max_power;
        unit
    }

    fn travel_form() -> Form {
        Form {
            form_id: 3,
            name: "Travel Form".into(),
            spell_id: 783,
            acts_as_shapeshifted: true,
            power_type: OWN_POOL,
            power_on_entry: PowerOnEntry::Keep as u8,
            default_for_class: 0,
            client_form: 3,
            natural_attack_ms: 0,
            attack_power_from_agility: false,
            clears_snares: true,
            entry_chance_spell: 0,
            blocks_mounting: true,
            alliance_display_id: 632,
            horde_display_id: 632,
            display_scale: 0.8,
        }
    }

    #[test]
    fn a_form_showing_the_own_pool_brings_a_druids_mana_back_from_reserve() {
        let mut druid = unit(DRUID, MANA, 700, 1000);
        druid.show_pool(ENERGY);
        druid.power = 60;
        show_form_pool(&mut druid, Some(&travel_form()));
        assert_eq!(
            (druid.shown_pool(), druid.reserve_pool()),
            (
                PowerPool {
                    power_type: MANA,
                    current: 700,
                    max: 1000
                },
                None
            )
        );
    }

    #[test]
    fn a_form_showing_the_own_pool_keeps_a_rogues_energy() {
        let mut rogue = unit(ROGUE, ENERGY, 80, 100);
        show_form_pool(&mut rogue, Some(&travel_form()));
        assert_eq!(
            (rogue.shown_pool(), rogue.reserve_pool()),
            (
                PowerPool {
                    power_type: ENERGY,
                    current: 80,
                    max: 100
                },
                None
            )
        );
    }
}
