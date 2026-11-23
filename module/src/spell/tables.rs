use spacetimedb::{table, Identity, ReducerContext, ScheduleAt, Table, Timestamp};

use super::CastRequest;
use super::{fire_pending_cast, fire_spell_impact, tick_auras, tick_ground_areas};
use crate::WorldEntity;
use lyracore_shared::form::DBC_MASK_FORMS;

const SPELL_ATTR_DO_NOT_DISPLAY: u32 = 0x80;
const SPELL_ATTR_NOT_SHAPESHIFT: u32 = 0x10000;
pub(crate) const SPELL_ATTR_NOT_IN_COMBAT_ONLY_PEACEFUL: u32 = 0x1000_0000;
const SPELL_ATTR_EX2_ALLOW_WHILE_NOT_SHAPESHIFTED: u32 = 0x80000;
// SPELLCATEGORY_GLOBAL: the StartRecoveryCategory of most spells with a global cooldown.
const GCD_CATEGORY_GLOBAL: u32 = 133;

#[table(accessor = game_spell, public)]
#[derive(Clone)]
pub struct Spell {
    #[primary_key]
    pub spell_id: u32,
    pub name: String,
    pub power_type: u8,
    pub cost: u32,
    pub cast_time_ms: u32,
    pub gcd_ms: u32,
    pub cooldown_ms: u32,
    pub range_yd: u32,
    pub duration_ms: u32,
    pub school_mask: u8,
    pub dispel_type: u8,
    pub mechanic: u8,
    pub max_stacks: u8,      // Zero and one both mean non-stacking.
    pub aura_interrupt: u16, // Bits 0 and 1 mean break on damage and movement.
    pub attributes: u32,
    pub spell_level: u8,
    pub max_level: u8,
    pub is_negative: bool,
    // Internal cast flags use a separate mask because Spell.dbc Attributes bits are occupied.
    #[default(0)]
    pub cast_flags: u32,
    // Unread; game_spell_form holds the Forms a spell requires or excludes.
    #[default(0)]
    pub stances: u8,
    #[default(0)]
    pub family_name: u8,
    #[default(0u64)]
    pub family_flags: u64,
    // Spell.dbc procFlags is stored verbatim.
    #[default(0u32)]
    pub proc_flags: u32,
    #[default(0u8)]
    pub proc_chance: u8,
    // Zero means unlimited charges.
    #[default(0u8)]
    pub proc_charges: u8,
    // A finisher's duration grows from duration_ms toward this at five combo points.
    #[default(0u32)]
    pub max_duration_ms: u32,
    #[default(0u32)]
    pub custom_flags: u32,
    #[default(0u32)]
    pub min_target_level: u32,
    #[default(0u64)]
    pub single_target_key: u64,
    // The verbatim Spell.dbc Stances mask; write_dbc_form_set turns it into game_spell_form rows.
    #[default(0u32)]
    pub form_mask: u32,
    #[default(0u32)]
    pub form_exclude: u32,
    #[default(0u32)]
    pub attributes_ex_2: u32,
    #[default(0u32)]
    pub visual_id: u32,
    #[default(0u32)]
    pub duration_index: u32,
    #[default(0u32)]
    pub category: u32,
    #[default(0u32)]
    pub category_cooldown_ms: u32,
    #[default(-1i32)]
    pub equipped_item_class: i32,
    #[default(0u32)]
    pub equipped_item_subclass_mask: u32,
    #[default(0u32)]
    pub equipped_item_inventory_mask: u32,
    #[default(0u32)]
    pub attributes_ex_3: u32,
    #[default(0u32)]
    pub attributes_ex_4: u32,
    #[default(0u8)]
    pub damage_class: u8,
    // Spell.dbc StartRecoveryCategory: the global cooldown this spell starts and waits on.
    #[default(0u32)]
    pub gcd_category: u32,
}

impl Spell {
    pub(crate) fn blank(spell_id: u32, name: &str) -> Self {
        Self {
            spell_id,
            name: name.to_string(),
            power_type: 0,
            cost: 0,
            cast_time_ms: 0,
            gcd_ms: 1500,
            cooldown_ms: 0,
            range_yd: 0,
            duration_ms: 0,
            school_mask: 0,
            dispel_type: 0,
            mechanic: 0,
            max_stacks: 0,
            aura_interrupt: 0,
            attributes: 0,
            spell_level: 0,
            max_level: 0,
            is_negative: false,
            cast_flags: 0,
            stances: 0,
            family_name: 0,
            family_flags: 0,
            proc_flags: 0,
            proc_chance: 0,
            proc_charges: 0,
            max_duration_ms: 0,
            custom_flags: 0,
            min_target_level: 0,
            single_target_key: 0,
            form_mask: 0,
            form_exclude: 0,
            attributes_ex_2: 0,
            visual_id: 0,
            duration_index: 0,
            category: 0,
            category_cooldown_ms: 0,
            equipped_item_class: -1,
            equipped_item_subclass_mask: 0,
            equipped_item_inventory_mask: 0,
            attributes_ex_3: 0,
            attributes_ex_4: 0,
            damage_class: 0,
            gcd_category: GCD_CATEGORY_GLOBAL,
        }
    }

    pub(crate) fn negative(&self) -> bool {
        lyracore_shared::spell_custom::is_negative(self.custom_flags, self.is_negative)
    }

    /// vmangos `SpellEntry::IsNonCombatSpell`.
    pub(crate) fn non_combat(&self) -> bool {
        self.attributes & SPELL_ATTR_NOT_IN_COMBAT_ONLY_PEACEFUL != 0
    }

    pub(crate) fn equip_passive(&self) -> bool {
        self.attributes & super::SPELL_ATTR_PASSIVE != 0
            || (self.attributes == SPELL_ATTR_DO_NOT_DISPLAY && self.duration_index == 21)
    }

    /// vmangos `SpellEntry::GetErrorAtShapeshiftedCast`, without its talent-learning exception. A
    /// passive held in any Form ignores its required Forms.
    pub(crate) fn form_refusal(
        &self,
        requirement: FormRequirement,
        form: Option<ShapeshiftState>,
    ) -> Option<FormRefusal> {
        if requirement.excludes_this {
            return Some(FormRefusal::NotShapeshift);
        }
        if requirement.requires_this {
            return None;
        }
        if form.is_some_and(|form| form.acts_as_shapeshifted) {
            if self.attributes & SPELL_ATTR_NOT_SHAPESHIFT != 0 {
                Some(FormRefusal::NotShapeshift)
            } else if requirement.requires_any && !self.held_in_any_form() {
                Some(FormRefusal::OnlyShapeshift)
            } else {
                None
            }
        } else if requirement.requires_any && !self.allowed_unshifted() {
            Some(FormRefusal::OnlyShapeshift)
        } else {
            None
        }
    }

    /// vmangos `SpellEntry::IsRemovedOnShapeLostSpell`.
    pub(crate) fn removed_on_shape_lost(&self, requirement: FormRequirement) -> bool {
        requirement.requires_any
            && !self.allowed_unshifted()
            && self.attributes & SPELL_ATTR_NOT_SHAPESHIFT == 0
    }

    /// A passive that only the Forms of its Form Set bring.
    pub(crate) fn binds_to_forms(&self, requirement: FormRequirement) -> bool {
        self.attributes & (super::SPELL_ATTR_PASSIVE | SPELL_ATTR_DO_NOT_DISPLAY) != 0
            && requirement.requires_any
            && !self.allowed_unshifted()
    }

    /// vmangos `SpellEntry::IsNeedCastSpellAtFormApply`: a known passive the Form brings with it.
    pub(crate) fn applies_on_form_entry(&self, requirement: FormRequirement) -> bool {
        self.binds_to_forms(requirement) && requirement.requires_this
    }

    /// A passive allowed while not shapeshifted, as the importer writes Furor: its Form Set scopes
    /// what it does, not where it can be held.
    fn held_in_any_form(&self) -> bool {
        self.attributes & super::SPELL_ATTR_PASSIVE != 0 && self.allowed_unshifted()
    }

    pub(crate) fn allowed_unshifted(&self) -> bool {
        self.attributes_ex_2 & SPELL_ATTR_EX2_ALLOW_WHILE_NOT_SHAPESHIFTED != 0
    }
}

/// The Form a unit is in, as the cast and equip Gates read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ShapeshiftState {
    pub(crate) form_id: u32,
    pub(crate) acts_as_shapeshifted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FormRefusal {
    NotShapeshift,
    OnlyShapeshift,
}

/// A Form a spell requires, or excludes when `excluded`. See docs/schema.md "Forms".
#[table(
    accessor = game_spell_form,
    public,
    index(accessor = by_spell, btree(columns = [spell_id]))
)]
pub struct SpellForm {
    /// `lyracore_package_delta::packed_spell_form_id(spell_id, form_id)`.
    #[primary_key]
    pub id: u64,
    pub spell_id: u32,
    pub form_id: u32,
    pub excluded: bool,
}

/// What a spell's Form Set says about one Form.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FormRequirement {
    pub(crate) requires_any: bool,
    pub(crate) requires_this: bool,
    pub(crate) excludes_this: bool,
}

impl FormRequirement {
    pub(crate) fn of(ctx: &ReducerContext, spell_id: u32, form_id: Option<u32>) -> Self {
        let mut requirement = Self::default();
        for row in ctx.db.game_spell_form().by_spell().filter(spell_id) {
            let this = Some(row.form_id) == form_id;
            if row.excluded {
                requirement.excludes_this |= this;
            } else {
                requirement.requires_any = true;
                requirement.requires_this |= this;
            }
        }
        requirement
    }
}

/// Replaces the Forms 1-32 of a spell's Form Set with those its DBC masks name. A Form in both
/// masks is excluded, our rule: vmangos reads the masks differently, and no 1.12 row sets both.
/// Rows for higher Form ids stay.
pub(crate) fn write_dbc_form_set(
    ctx: &ReducerContext,
    spell_id: u32,
    required: u32,
    excluded: u32,
) {
    let rows = ctx.db.game_spell_form();
    let stale: Vec<u64> = rows
        .by_spell()
        .filter(spell_id)
        .filter(|row| DBC_MASK_FORMS.contains(&row.form_id))
        .map(|row| row.id)
        .collect();
    for id in stale {
        rows.id().delete(id);
    }
    for form_id in DBC_MASK_FORMS {
        let bit = 1u32 << (form_id - 1);
        if (required | excluded) & bit != 0 {
            rows.insert(SpellForm {
                id: lyracore_package_delta::packed_spell_form_id(spell_id, form_id),
                spell_id,
                form_id,
                excluded: excluded & bit != 0,
            });
        }
    }
}

/// Removes every row of a deleted spell's Form Set.
pub(crate) fn delete_form_set(ctx: &ReducerContext, spell_id: u32) {
    let rows = ctx.db.game_spell_form();
    let ids: Vec<u64> = rows.by_spell().filter(spell_id).map(|row| row.id).collect();
    for id in ids {
        rows.id().delete(id);
    }
}

/// Publish repair: a spell whose DBC masks name Forms but which has no Form Set rows gets them.
pub(crate) fn backfill_dbc_form_sets(ctx: &ReducerContext) -> u64 {
    let pending: Vec<(u32, u32, u32)> = ctx
        .db
        .game_spell()
        .iter()
        .filter(|spell| spell.form_mask | spell.form_exclude != 0)
        .filter(|spell| {
            ctx.db
                .game_spell_form()
                .by_spell()
                .filter(spell.spell_id)
                .next()
                .is_none()
        })
        .map(|spell| (spell.spell_id, spell.form_mask, spell.form_exclude))
        .collect();
    for &(spell_id, required, excluded) in &pending {
        write_dbc_form_set(ctx, spell_id, required, excluded);
    }
    pending.len() as u64
}

#[table(
    accessor = game_spell_effect,
    public,
    index(accessor = by_spell, btree(columns = [spell_id])),
    index(accessor = by_trigger_spell, btree(columns = [trigger_spell]))
)]
pub struct SpellEffect {
    #[primary_key]
    pub id: u64, // Deterministic id = (spell_id << 2) | effect_index.
    pub spell_id: u32,
    pub effect_index: u8,
    pub kind: u8,
    pub base_points: i32, // The importer already added the Spell.dbc base-point offset of one.
    pub die_sides: i32,
    pub per_level: f32,
    pub period_ms: u32,
    pub target: u8,
    pub radius_yd: f32,
    pub chain_targets: u8,
    pub trigger_spell: u32,
    pub effect_mechanic: u8, // Zero inherits the spell header mechanic.
    pub p0: i32,
    pub p0_kind: u8,
    pub p1: i32,
    pub script_id: u32,
    #[default(false)]
    pub enters_combat: bool,
    #[default(0.0f32)]
    pub points_per_combo: f32,
    #[default({ -1.0f32 })]
    pub bonus_coefficient: f32,
    // Reimport spell before enabling derived equip effects; see docs/danger-zones.md.
    #[default(false)]
    pub equip_supported: bool,
}

impl SpellEffect {
    pub(crate) fn blank(spell_id: u32, effect_index: u8) -> Self {
        Self {
            id: lyracore_package_delta::packed_spell_effect_id(spell_id, effect_index),
            spell_id,
            effect_index,
            kind: 0,
            base_points: 0,
            die_sides: 0,
            per_level: 0.0,
            period_ms: 0,
            target: 0,
            radius_yd: 0.0,
            chain_targets: 0,
            trigger_spell: 0,
            effect_mechanic: 0,
            p0: 0,
            p0_kind: 0,
            p1: 0,
            script_id: 0,
            enters_combat: false,
            points_per_combo: 0.0,
            bonus_coefficient: -1.0,
            equip_supported: false,
        }
    }
}

#[table(accessor = game_spell_reagent, index(accessor = by_spell, btree(columns = [spell_id])))]
pub struct SpellReagent {
    #[primary_key]
    pub id: u64, // Deterministic id = (spell_id << 3) | reagent_slot.
    pub spell_id: u32,
    pub item_entry: u32,
    pub count: u32,
}

/// The spell a caster casts as a Follow-up Cast at each unit its spell hits.
#[table(accessor = game_spell_follow_up)]
pub struct SpellFollowUp {
    #[primary_key]
    pub spell_id: u32,
    pub follow_up_spell_id: u32,
}

// vmangos `spell_threat`, which every rank without its own row takes from its first rank.
#[table(accessor = game_spell_threat)]
pub struct SpellThreat {
    #[primary_key]
    pub spell_id: u32,
    pub threat: u16,
    // Scales the spell's damage and heal threat.
    pub multiplier: f32,
    // Effect indexes that do not carry the flat threat to their target.
    pub inverse_effect_mask: u8,
}

#[table(accessor = game_spell_proc_event)]
pub struct SpellProcEvent {
    #[primary_key]
    pub spell_id: u32,
    pub proc_flags: u32,
    pub proc_ex: u32,
    pub school_mask: u8,
    pub family_name: u8,
    pub family_flags: u64,
    pub ppm_rate: f32,
    pub custom_chance: u8,
    pub icd_ms: u32,
}

#[table(
    accessor = game_aura,
    public,
    index(accessor = by_target, btree(columns = [target_guid])),
    index(accessor = by_caster, btree(columns = [caster_guid])),
    index(accessor = by_expiry, btree(columns = [expires_at])),
    index(accessor = by_kind_param, btree(columns = [eff_kind, eff_p0])),
    index(accessor = by_next_tick, btree(columns = [next_tick_micros]))
)]
pub struct Aura {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub target_guid: u64,
    pub caster_guid: u64,
    pub spell_id: u32,
    pub slot: u8,
    pub level: u8,
    pub flags: u8,
    pub applied_at: Timestamp,
    pub expires_at: Timestamp,
    #[default(0u64)]
    pub effect_id: u64,
    #[default(0)]
    pub eff_kind: u8,
    #[default(0)]
    pub amount: i32,
    #[default(0)]
    pub eff_p0: i32,
    #[default(0)]
    pub eff_p0_kind: u8,
    #[default(0)]
    pub eff_p1: i32,
    #[default(0)]
    pub period_ms: u32,
    #[default(0)]
    pub amount_remaining: i32,
    #[default(0)]
    pub stacks: u8,
    // Unix microseconds; zero means no pending tick.
    #[default(0i64)]
    pub next_tick_micros: i64,
    // Cast target for channels; missile target for periodic triggers.
    #[default(0u64)]
    pub channel_target: u64,
    #[default(false)]
    pub enters_combat: bool,
    #[default(0u32)]
    pub proc_flags: u32,
    #[default(0u8)]
    pub proc_chance: u8,
    #[default(0.0f32)]
    pub proc_ppm: f32,
    #[default(0u32)]
    pub proc_ex: u32,
    #[default(0u8)]
    pub proc_school_mask: u8,
    #[default(0u8)]
    pub proc_family_name: u8,
    #[default(0u64)]
    pub proc_family_flags: u64,
    // Zero means unlimited charges.
    #[default(0u8)]
    pub proc_charges: u8,
    #[default(0u32)]
    pub proc_icd_ms: u32,
    #[default(0i64)]
    pub proc_ready_micros: i64,
}

#[table(accessor = game_item_aura)]
struct ItemAura {
    #[primary_key]
    aura_id: u64,
    item_guid: u64,
    definition: Vec<u8>,
}

#[table(accessor = game_item_aura_expiry, scheduled(expire_item_aura))]
pub struct ItemAuraExpiry {
    #[primary_key]
    pub aura_id: u64,
    pub scheduled_at: ScheduleAt,
}

crate::outside_table_catalog!(
    game_item_aura,
    game_item_aura_expiry,
    game_spell_form,
    game_spell_threat
);

#[spacetimedb::reducer]
pub(crate) fn expire_item_aura(ctx: &ReducerContext, expiry: ItemAuraExpiry) {
    if !crate::time::scheduler_authorized(ctx) {
        return;
    }
    if let Some(aura) = ctx.db.game_aura().id().find(expiry.aura_id) {
        if aura.expires_at <= crate::time::now(ctx) {
            shed_auras(ctx, vec![aura]);
        }
    }
}

impl Aura {
    fn item_definition(ctx: &ReducerContext, spell: u32, effect: u64) -> Option<Vec<u8>> {
        let spell = ctx.db.game_spell().spell_id().find(spell)?;
        let effect = ctx.db.game_spell_effect().id().find(effect)?;
        spacetimedb::sats::bsatn::to_vec(&(spell, effect)).ok()
    }

    pub(crate) fn item_definition_matches(&self, ctx: &ReducerContext) -> bool {
        ctx.db
            .game_item_aura()
            .aura_id()
            .find(self.id)
            .is_some_and(|source| {
                Self::item_definition(ctx, self.spell_id, self.effect_id)
                    .is_some_and(|definition| definition == source.definition)
            })
    }

    fn arm_item_expiry(&self, ctx: &ReducerContext) {
        ctx.db.game_item_aura_expiry().aura_id().delete(self.id);
        if self.item_guid(ctx) != 0 && self.expires_at.to_micros_since_unix_epoch() != i64::MAX {
            ctx.db.game_item_aura_expiry().insert(ItemAuraExpiry {
                aura_id: self.id,
                scheduled_at: crate::time::schedule_time(ctx, self.expires_at),
            });
        }
    }

    pub(crate) fn item_guid(&self, ctx: &ReducerContext) -> u64 {
        ctx.db
            .game_item_aura()
            .aura_id()
            .find(self.id)
            .map_or(0, |source| source.item_guid)
    }

    pub(crate) fn insert(self, ctx: &ReducerContext) -> Self {
        self.insert_from_item(ctx, 0)
    }

    pub(crate) fn insert_from_item(self, ctx: &ReducerContext, item_guid: u64) -> Self {
        let aura = ctx.db.game_aura().insert(self);
        if item_guid != 0 {
            ctx.db.game_item_aura().insert(ItemAura {
                aura_id: aura.id,
                item_guid,
                definition: Self::item_definition(ctx, aura.spell_id, aura.effect_id)
                    .unwrap_or_default(),
            });
            aura.arm_item_expiry(ctx);
        }
        if aura.eff_kind == lyracore_shared::spell::taxonomy::A_STEALTH {
            crate::aoi::refresh_entity(ctx, aura.target_guid);
        }
        crate::skill::aura_changed(ctx, None, Some(&aura));
        crate::native::aura::changed(ctx, None, Some(&aura));
        aura
    }

    pub(crate) fn update(self, ctx: &ReducerContext) -> Self {
        let auras = ctx.db.game_aura();
        let previous = auras.id().find(self.id);
        let aura = auras.id().update(self);
        if previous
            .as_ref()
            .is_some_and(|old| old.expires_at != aura.expires_at)
        {
            aura.arm_item_expiry(ctx);
        }
        if previous
            .as_ref()
            .is_some_and(|old| old.eff_kind != aura.eff_kind)
        {
            crate::aoi::refresh_entity(ctx, aura.target_guid);
        }
        crate::skill::aura_changed(ctx, previous.as_ref(), Some(&aura));
        crate::native::aura::changed(ctx, previous.as_ref(), Some(&aura));
        aura
    }

    /// Removes one Aura and refreshes what it derived; `shed_auras` removes a batch.
    #[cfg_attr(not(has_packages), allow(dead_code))]
    pub(crate) fn delete(ctx: &ReducerContext, id: u64) -> bool {
        let previous = ctx.db.game_aura().id().find(id);
        let removed = previous.is_some();
        match previous {
            Some(aura) => shed_auras(ctx, vec![aura]),
            None => {
                Self::delete_row(ctx, id);
            }
        }
        removed
    }

    fn delete_row(ctx: &ReducerContext, id: u64) -> Option<Aura> {
        let auras = ctx.db.game_aura();
        let previous = auras.id().find(id);
        auras.id().delete(id);
        ctx.db.game_item_aura().aura_id().delete(id);
        ctx.db.game_item_aura_expiry().aura_id().delete(id);
        if let Some(aura) = &previous {
            if aura.eff_kind == lyracore_shared::spell::taxonomy::A_STEALTH {
                crate::aoi::refresh_entity(ctx, aura.target_guid);
            }
            crate::skill::aura_changed(ctx, Some(aura), None);
            crate::native::aura::changed(ctx, Some(aura), None);
        }
        previous
    }
}

/// The unit columns an Aura's effect feeds, which must be recomputed when it changes.
#[derive(Clone, Copy, Default)]
pub(crate) struct AuraProjections {
    vitals: bool,
    sheet: bool,
    mount: bool,
    display: bool,
    form: bool,
}

impl AuraProjections {
    pub(crate) fn of(spell_id: u32, kind: u8, parameter: i32) -> Self {
        Self {
            vitals: crate::modifiers::moves_vitals(kind, parameter),
            sheet: crate::modifiers::moves_sheet(spell_id, kind, parameter),
            mount: crate::mount::mount_aura_moves_mount(kind, parameter),
            display: kind == lyracore_shared::spell::taxonomy::A_TRANSFORM,
            form: kind == lyracore_shared::spell::taxonomy::A_SHAPESHIFT,
        }
    }

    fn union(self, other: Self) -> Self {
        Self {
            vitals: self.vitals || other.vitals,
            sheet: self.sheet || other.sheet,
            mount: self.mount || other.mount,
            display: self.display || other.display,
            form: self.form || other.form,
        }
    }

    pub(crate) fn refresh(self, ctx: &ReducerContext, unit_guid: u64) {
        if self.vitals {
            super::recompute_vitals(ctx, unit_guid);
        }
        if self.sheet {
            super::recompute_sheet(ctx, unit_guid);
        }
        if self.mount {
            crate::mount::recompute_mount(ctx, unit_guid);
        }
        if self.display {
            super::recompute_display(ctx, unit_guid);
        }
        if self.form {
            crate::form::project_form(ctx, unit_guid);
        }
    }
}

/// Removes every Aura in `auras`, opens the diminishing-returns window of each control category
/// that no longer holds the unit, and refreshes each touched unit's derived columns once.
pub(crate) fn shed_auras(ctx: &ReducerContext, auras: Vec<Aura>) {
    shed(ctx, auras, true);
}

/// As `shed_auras`, for a caller that projects the Form itself afterwards.
pub(crate) fn shed_auras_before_projection(ctx: &ReducerContext, auras: Vec<Aura>) {
    shed(ctx, auras, false);
}

fn shed(ctx: &ReducerContext, auras: Vec<Aura>, project_form: bool) {
    let now_micros = crate::time::now(ctx).to_micros_since_unix_epoch();
    let mut touched = std::collections::BTreeMap::<u64, AuraProjections>::new();
    let mut exited = Vec::new();
    for aura in auras {
        let Some(aura) = Aura::delete_row(ctx, aura.id) else {
            continue;
        };
        if let Some(category) = super::stacking::dr_category_for_effect(
            ctx,
            aura.eff_kind,
            aura.eff_p0_kind,
            aura.eff_p0,
            aura.target_guid,
        ) {
            super::stacking::dr_window_on_removal(ctx, aura.target_guid, category, now_micros);
        }
        let projections = touched.entry(aura.target_guid).or_default();
        *projections = projections.union(AuraProjections::of(
            aura.spell_id,
            aura.eff_kind,
            aura.eff_p0,
        ));
        if aura.eff_kind == lyracore_shared::spell::taxonomy::A_SHAPESHIFT {
            exited.push(aura);
        }
    }
    for aura in exited {
        crate::form::exited(ctx, &aura);
    }
    for (unit_guid, mut projections) in touched {
        projections.form &= project_form;
        projections.refresh(ctx, unit_guid);
    }
}

pub(crate) fn auras_on(ctx: &ReducerContext, unit_guid: u64) -> impl Iterator<Item = Aura> {
    ctx.db.game_aura().by_target().filter(unit_guid)
}

pub(crate) fn is_stealthed(ctx: &ReducerContext, unit_guid: u64) -> bool {
    auras_on(ctx, unit_guid).any(|a| a.eff_kind == lyracore_shared::spell::taxonomy::A_STEALTH)
}

/// Ends every Aura of each spell that stealths the unit, as vmangos removes the whole holder, so
/// Stealth's Form and slow end with it.
pub(crate) fn break_stealth(ctx: &ReducerContext, unit_guid: u64) {
    shed_auras(ctx, stealth_spell_auras(ctx, unit_guid, 0));
}

/// The unit's Auras of each spell, other than `except_spell`, that holds an `A_STEALTH` Aura on it.
pub(crate) fn stealth_spell_auras(
    ctx: &ReducerContext,
    unit_guid: u64,
    except_spell: u32,
) -> Vec<Aura> {
    let spells: std::collections::BTreeSet<u32> = auras_on(ctx, unit_guid)
        .filter(|a| {
            a.eff_kind == lyracore_shared::spell::taxonomy::A_STEALTH && a.spell_id != except_spell
        })
        .map(|a| a.spell_id)
        .collect();
    if spells.is_empty() {
        return Vec::new();
    }
    auras_on(ctx, unit_guid)
        .filter(|a| spells.contains(&a.spell_id))
        .collect()
}

pub(crate) fn remove_seal_auras(ctx: &ReducerContext, unit_guid: u64) -> u32 {
    let seal_spell_ids: std::collections::HashSet<u32> = auras_on(ctx, unit_guid)
        .filter(|a| a.eff_kind == lyracore_shared::spell::taxonomy::A_SEAL)
        .map(|a| a.spell_id)
        .collect();
    let seals: Vec<Aura> = auras_on(ctx, unit_guid)
        .filter(|a| seal_spell_ids.contains(&a.spell_id))
        .collect();
    let removed = seals.len() as u32;
    shed_auras(ctx, seals);
    removed
}

crate::character_owned!(delete, fn sweep_delete_game_aura(ctx, character_guid) {
    let auras = ctx.db.game_aura();
    let ids: Vec<u64> = auras.by_target().filter(&character_guid).map(|a| a.id).collect();
    for id in ids {
        Aura::delete_row(ctx, id);
    }
});
crate::character_owned!(transfer, fn sweep_transfer_game_aura(ctx, character_guid, io) {
    crate::transfer::move_rows(
        ctx, io,
        || ctx.db.game_aura().by_target().filter(character_guid)
            .filter(|aura| aura.item_guid(ctx) == 0).collect(),
        |ctx, mut aura: Aura| { aura.id = 0; aura.insert(ctx); },
    );
});

pub const CAST_FAIL_NONE: u8 = 0;
pub const CAST_FAIL_NO_POWER: u8 = super::CastRefusalReason::NoPower.failure_code();
pub const CAST_FAIL_INTERRUPTED: u8 = super::CastRefusalReason::Interrupted.failure_code();

// Codes are append-only for rolling Module updates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum SpellCastEventKind {
    Start = 1,
    Go = 2,
    Interrupt = 3,
    Pushback = 4,
    ProcLog = 5,
    PeriodicLog = 6,
}

#[table(
    accessor = game_spell_cast_event,
    public,
    index(accessor = by_grid, btree(columns = [map_id, instance_id, grid_x, grid_y]))
)]
#[table(accessor = game_spell_cast_event_live, public, event)]
pub struct SpellCastEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub caster_guid: u64,
    pub spell_id: u32,
    pub created_at: Timestamp,
    #[default(0u64)]
    pub target_guid: u64,
    #[default(0u32)]
    pub cast_time_ms: u32,
    // Timed completions and queued strike GO events must not emit a second START.
    #[default(false)]
    pub is_completion: bool,
    #[default(0u32)]
    pub damage: u32,
    // SpellSchool index 0..=6, rather than a school bitmask.
    #[default(0u8)]
    pub school: u8,
    #[default(false)]
    pub is_crit: bool,
    #[default(0u32)]
    pub resisted: u32,
    #[default(0u32)]
    pub absorbed: u32,
    #[default(false)]
    pub is_interrupted: bool,
    // Zero-duration SMSG_SPELL_COOLDOWN leaves the 1.12 client action button stuck.
    #[default(0u32)]
    pub cooldown_ms: u32,
    #[default(0u32)]
    pub delay_ms: u32,
    #[default(0u32)]
    pub healed: u32,
    #[default(false)]
    pub is_proc_log: bool,
    #[default(0u8)]
    pub swing_hit_info: u8,
    // The caster's own cast handler already sent it START and CAST_RESULT.
    #[default(false)]
    pub client_initiated: bool,
    // SpacetimeDB encodes an untyped zero as u32; u64 defaults must be typed.
    #[default(0u32)]
    pub map_id: u32,
    #[default(0u64)]
    pub instance_id: u64,
    #[default(0i32)]
    pub grid_x: i32,
    #[default(0i32)]
    pub grid_y: i32,
    // A `CastRefusalReason` code; its failed SMSG_CAST_RESULT releases the queued melee ability.
    #[default(0u8)]
    pub failure_reason: u8,
    // Zero selects legacy field decoding; new rows use nonzero codes.
    #[default(0u8)]
    pub kind: u8,
    // `CAST_MISS_*` for the primary target on a GO row.
    #[default(0u8)]
    pub miss_info: u8,
    // Power given to the primary target; `power_type` is the target's power byte.
    #[default(0u32)]
    pub energized: u32,
    #[default(0u8)]
    pub power_type: u8,
    // Damage a queued strike's partial block took; only a queued strike writes it.
    #[default(0u32)]
    pub blocked_amount: u32,
    // A `StrikeSwingReport` code; a queued strike's GO row also reports its swing.
    #[default(0u8)]
    pub strike_swing_report: u8,
}

/// How a GO row reports a queued strike's swing (vmangos Spell::SendMeleeAttackingStateUpdate).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum StrikeSwingReport {
    None = 0,
    WithDamage = 1,
    /// SPELL_ATTR_ON_NEXT_SWING_NO_DAMAGE: the swing's damage fields stay zero.
    WithoutDamage = 2,
}

impl StrikeSwingReport {
    pub(crate) fn from_code(code: u8) -> Self {
        match code {
            1 => Self::WithDamage,
            2 => Self::WithoutDamage,
            _ => Self::None,
        }
    }
}

pub const CAST_MISS_NONE: u8 = 0;
pub const CAST_MISS_MISS: u8 = 1;
/// Vanilla reports a magic spell's failed hit roll as a full resist.
pub const CAST_MISS_RESIST: u8 = 2;
pub const CAST_MISS_DODGE: u8 = 3;
pub const CAST_MISS_PARRY: u8 = 4;
pub const CAST_MISS_IMMUNE: u8 = 7;
pub const CAST_MISS_IMMUNE2: u8 = 8;

impl SpellImpactEvent {
    pub(crate) fn insert(ctx: &ReducerContext, row: Self) -> Self {
        crate::native::spell::spell_impact(ctx, &row);
        ctx.db.game_spell_impact_event_live().insert(row)
    }
}

impl SpellCastEvent {
    pub(crate) fn insert(ctx: &ReducerContext, row: Self) -> Self {
        Self::insert_at(ctx, row, crate::native::spell::GoDelivery::Pending)
    }

    /// Inserts a row; `delivery` says whether its GO packets went out already.
    pub(crate) fn insert_at(
        ctx: &ReducerContext,
        row: Self,
        delivery: crate::native::spell::GoDelivery,
    ) -> Self {
        crate::native::spell::cast_event(ctx, &row, delivery);
        ctx.db.game_spell_cast_event_live().insert(row)
    }

    pub(crate) fn signal(
        ctx: &ReducerContext,
        caster_guid: u64,
        spell_id: u32,
        kind: SpellCastEventKind,
    ) -> Self {
        Self::signal_addr(
            ctx,
            caster_guid,
            spell_id,
            crate::helpers::grid_of(ctx, caster_guid),
            kind,
        )
    }

    pub(crate) fn signal_at(
        ctx: &ReducerContext,
        caster: &WorldEntity,
        spell_id: u32,
        kind: SpellCastEventKind,
    ) -> Self {
        Self::signal_addr(
            ctx,
            caster.guid,
            spell_id,
            crate::helpers::entity_addr(caster),
            kind,
        )
    }

    /// Addressed at the target, since the caster may be far away or gone.
    pub(crate) fn periodic_log(
        ctx: &ReducerContext,
        caster_guid: u64,
        spell_id: u32,
        target: &WorldEntity,
    ) -> Self {
        Self {
            target_guid: target.guid,
            ..Self::signal_addr(
                ctx,
                caster_guid,
                spell_id,
                crate::helpers::entity_addr(target),
                SpellCastEventKind::PeriodicLog,
            )
        }
    }

    fn signal_addr(
        ctx: &ReducerContext,
        caster_guid: u64,
        spell_id: u32,
        address: (u32, u64, i32, i32),
        kind: SpellCastEventKind,
    ) -> Self {
        let (map_id, instance_id, grid_x, grid_y) = address;
        Self {
            id: 0,
            caster_guid,
            spell_id,
            created_at: crate::time::now(ctx),
            target_guid: 0,
            cast_time_ms: 0,
            is_completion: false,
            damage: 0,
            school: 0,
            is_crit: false,
            resisted: 0,
            absorbed: 0,
            is_interrupted: false,
            cooldown_ms: 0,
            delay_ms: 0,
            healed: 0,
            is_proc_log: false,
            swing_hit_info: 0,
            client_initiated: false,
            map_id,
            instance_id,
            grid_x,
            grid_y,
            failure_reason: CAST_FAIL_NONE,
            kind: kind as u8,
            miss_info: CAST_MISS_NONE,
            energized: 0,
            power_type: 0,
            blocked_amount: 0,
            strike_swing_report: 0,
        }
    }
}

#[table(
    accessor = game_global_cooldown,
    public,
    index(accessor = by_caster_category, btree(columns = [caster_guid, category]))
)]
pub struct GlobalCooldown {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub caster_guid: u64,
    pub category: u32,
    pub ready_at: Timestamp,
}

#[table(accessor = game_spell_cd, public, index(accessor = by_caster, btree(columns = [caster_guid])))]
pub struct SpellCd {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub caster_guid: u64,
    pub spell_id: u32,
    pub ready_at: Timestamp,
    #[default(0u32)]
    pub category: u32,
    #[default(0i64)]
    pub category_ready_micros: i64,
}

crate::character_owned!(delete, fn sweep_delete_game_spell_cd(ctx, character_guid) {
    let cooldowns = ctx.db.game_spell_cd();
    for cooldown in cooldowns.by_caster().filter(character_guid).collect::<Vec<_>>() {
        cooldowns.id().delete(cooldown.id);
    }
});
crate::character_owned!(transfer, fn sweep_transfer_game_spell_cd(ctx, character_guid, io) {
    table = game_spell_cd,
    by = by_caster,
    remint = id,
});

#[table(accessor = game_school_lockout, public, index(accessor = by_caster, btree(columns = [caster_guid])))]
pub struct SchoolLockout {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub caster_guid: u64,
    pub school: u8,
    pub until: Timestamp,
}

#[table(accessor = game_aura_schedule, scheduled(tick_auras))]
pub struct AuraSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}

#[table(accessor = game_ground_area)]
pub struct GroundArea {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub spell_id: u32,
    pub caster_guid: u64,
    pub map_id: u32,
    pub instance_id: u64,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub radius_yd: f32,
    pub amount: i32,
    pub school_mask: u8,
    pub period_ms: u32,
    pub next_tick_micros: i64,
    pub expires_at: Timestamp,
    #[default(0)]
    pub effect_id: u64,
}

// Client build 5875 draws the ground effect from a DynamicObject CREATE.
#[table(accessor = game_dynamic_object, public,
    index(accessor = by_cell, btree(columns = [map_id, instance_id, cell]))
)]
pub struct DynamicObject {
    #[primary_key]
    pub guid: u64,
    pub caster_guid: u64,
    pub spell_id: u32,
    pub map_id: u32,
    pub instance_id: u64,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub radius_yd: f32,
    #[default(0i64)]
    pub cell: i64,
}

impl DynamicObject {
    pub(crate) fn position_cell(&self) -> i64 {
        lyracore_shared::spatial::cell_id_at(self.x, self.y)
    }

    pub(crate) fn insert(mut self, ctx: &ReducerContext) {
        self.cell = self.position_cell();
        let object = ctx.db.game_dynamic_object().insert(self);
        #[cfg(feature = "conformance_test_fixture")]
        crate::native::debug::record_birth(
            ctx,
            object.guid,
            object.map_id,
            object.instance_id,
            object.cell,
        );
        crate::aoi::dynamic_object_changed(ctx, &object, true);
    }

    #[cfg(feature = "debug_reducers")]
    pub(crate) fn repair_cell(mut self, ctx: &ReducerContext) {
        self.cell = self.position_cell();
        let object = ctx.db.game_dynamic_object().guid().update(self);
        crate::aoi::dynamic_object_changed(ctx, &object, false);
    }

    pub(crate) fn delete(ctx: &ReducerContext, guid: u64) {
        ctx.db.game_dynamic_object().guid().delete(guid);
        crate::aoi::remove_subject(ctx, crate::aoi::DYNAMIC_OBJECT, guid);
    }
}

#[table(accessor = game_ground_area_schedule, scheduled(tick_ground_areas))]
pub struct GroundAreaSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}

#[table(
    accessor = game_pending_cast,
    scheduled(fire_pending_cast),
    index(accessor = by_caster, btree(columns = [caster_guid]))
)]
pub struct PendingCast {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    pub caster_guid: u64,
    pub spell_id: u32,
    pub level: u8,
    pub target_guid: u64,
    #[default(0)]
    pub pushback_count: u8,
    // has_dest distinguishes an absent destination from the valid point 0,0,0.
    #[default(false)]
    pub has_dest: bool,
    #[default(0.0)]
    pub dest_x: f32,
    #[default(0.0)]
    pub dest_y: f32,
    #[default(0.0)]
    pub dest_z: f32,
    // Zero is no item target; item guids start above zero.
    #[default(0u64)]
    pub item_target_guid: u64,
    #[default(None::<u32>)]
    pub cooldown_category: Option<u32>,
    // Zero is no cast item; item guids start above zero.
    #[default(0u64)]
    pub cast_item_guid: u64,
}

impl PendingCast {
    pub(crate) fn scheduled(request: &CastRequest, scheduled_at: ScheduleAt) -> Self {
        let (has_dest, (dest_x, dest_y, dest_z)) = match request.dest {
            Some(point) => (true, point),
            None => (false, (0.0, 0.0, 0.0)),
        };
        Self {
            scheduled_id: 0,
            scheduled_at,
            caster_guid: request.caster_guid,
            spell_id: request.spell_id,
            level: request.level,
            target_guid: request.target_guid,
            pushback_count: 0,
            has_dest,
            dest_x,
            dest_y,
            dest_z,
            item_target_guid: request.item_target.unwrap_or(0),
            cooldown_category: request.cooldown_category,
            cast_item_guid: request.cast_item.unwrap_or(0),
        }
    }

    /// The stored request. The row keeps no client initiation, so a completion reports none.
    pub(crate) fn request(&self) -> CastRequest {
        CastRequest {
            caster_guid: self.caster_guid,
            spell_id: self.spell_id,
            level: self.level,
            target_guid: self.target_guid,
            client_initiated: false,
            dest: self
                .has_dest
                .then_some((self.dest_x, self.dest_y, self.dest_z)),
            item_target: (self.item_target_guid != 0).then_some(self.item_target_guid),
            cast_item: (self.cast_item_guid != 0).then_some(self.cast_item_guid),
            cooldown_category: self.cooldown_category,
        }
    }
}

#[table(accessor = game_pending_spell_impact, scheduled(fire_spell_impact))]
pub struct PendingSpellImpact {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    pub caster_guid: u64,
    pub target_guid: u64,
    pub spell_id: u32,
    pub school_index: u8,
    pub is_crit: bool,
    pub resisted: u32,
    pub after_resist: i32, // Rolled at cast time, after crit and resist but before absorb.
    // Preserve triggered origin across travel time so a proc missile cannot trigger more procs.
    #[default(false)]
    pub triggered: bool,
}

#[table(
    accessor = game_spell_impact_event,
    public,
    index(accessor = by_grid, btree(columns = [map_id, instance_id, grid_x, grid_y]))
)]
#[table(accessor = game_spell_impact_event_live, public, event)]
pub struct SpellImpactEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub caster_guid: u64,
    pub target_guid: u64,
    pub spell_id: u32,
    pub created_at: Timestamp,
    pub damage: u32,
    pub school: u8,
    pub is_crit: bool,
    pub resisted: u32,
    pub absorbed: u32,
    #[default(0u32)]
    pub map_id: u32,
    #[default(0u64)]
    pub instance_id: u64,
    #[default(0i32)]
    pub grid_x: i32,
    #[default(0i32)]
    pub grid_y: i32,
}

#[table(accessor = game_resurrect_request, public)]
pub struct ResurrectRequest {
    #[primary_key]
    pub target_guid: u64,
    pub target_identity: Identity,
    pub caster_guid: u64,
    pub caster_name: String,
    pub points: i32, // E_RESURRECT percent frozen when offered, rather than when accepted.
    pub created_at: Timestamp,
}

impl ResurrectRequest {
    pub(crate) fn insert(self, ctx: &ReducerContext) {
        let requests = ctx.db.game_resurrect_request();
        requests.target_guid().delete(self.target_guid);
        let row = requests.insert(self);
        crate::native::death::resurrect_offered(ctx, &row);
    }

    pub(crate) fn delete(ctx: &ReducerContext, guid: u64) {
        ctx.db.game_resurrect_request().target_guid().delete(guid);
    }
}

#[table(accessor = game_self_resurrect_option, public)]
pub struct SelfResurrectOption {
    #[primary_key]
    pub character_guid: u64,
    pub spell_id: u32,
}

impl SelfResurrectOption {
    pub(crate) fn set(ctx: &ReducerContext, guid: u64, spell_id: Option<u32>) {
        let options = ctx.db.game_self_resurrect_option();
        if options.character_guid().find(guid).map(|row| row.spell_id) == spell_id {
            return;
        }
        options.character_guid().delete(guid);
        if let Some(spell_id) = spell_id {
            options.insert(SelfResurrectOption {
                character_guid: guid,
                spell_id,
            });
        }
        crate::native::death::self_resurrection_changed(ctx, guid, spell_id.unwrap_or(0));
    }
}

#[cfg(test)]
mod pending_cast_tests {
    use super::*;

    fn round_trip(request: CastRequest) -> CastRequest {
        let at = ScheduleAt::Time(Timestamp::from_micros_since_unix_epoch(1_000_000));
        PendingCast::scheduled(&request, at).request()
    }

    #[test]
    fn a_pending_cast_keeps_its_ground_point_items_and_cooldown_category() {
        let request = CastRequest {
            dest: Some((0.0, 0.0, 0.0)),
            item_target: Some(7),
            cast_item: Some(9),
            cooldown_category: Some(11),
            ..CastRequest::new(1, 2, 3, 4)
        };
        assert_eq!(round_trip(request), request);
        assert_eq!(
            round_trip(CastRequest::new(1, 2, 3, 1)),
            CastRequest::new(1, 2, 3, 1)
        );
    }

    #[test]
    fn a_pending_cast_does_not_keep_client_initiation() {
        let request = CastRequest {
            client_initiated: true,
            ..CastRequest::new(1, 2, 3, 4)
        };
        assert_eq!(round_trip(request), CastRequest::new(1, 2, 3, 4));
    }
}
