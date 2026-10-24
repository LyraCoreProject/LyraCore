use spacetimedb::{table, Identity, ReducerContext, ScheduleAt, Table, Timestamp};

use super::{fire_pending_cast, fire_spell_impact, tick_auras, tick_ground_areas};
use crate::WorldEntity;

#[table(accessor = game_spell, public)]
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
    // Internal stance bits are Battle=1, Defensive=2, Berserker=4; zero permits every stance.
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
}

#[table(accessor = game_spell_reagent, index(accessor = by_spell, btree(columns = [spell_id])))]
pub struct SpellReagent {
    #[primary_key]
    pub id: u64, // Deterministic id = (spell_id << 3) | reagent_slot.
    pub spell_id: u32,
    pub item_entry: u32,
    pub count: u32,
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
    // Channel auras target the caster; this guid holds the missile destination.
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

impl Aura {
    pub(crate) fn insert(self, ctx: &ReducerContext) -> Self {
        let aura = ctx.db.game_aura().insert(self);
        if aura.eff_kind == lyracore_shared::spell::taxonomy::A_STEALTH {
            crate::aoi::refresh_entity(ctx, aura.target_guid);
        }
        crate::native::aura_changed(ctx, None, Some(&aura));
        aura
    }

    pub(crate) fn update(self, ctx: &ReducerContext) -> Self {
        let auras = ctx.db.game_aura();
        let previous = auras.id().find(self.id);
        let aura = auras.id().update(self);
        if previous
            .as_ref()
            .is_some_and(|old| old.eff_kind != aura.eff_kind)
        {
            crate::aoi::refresh_entity(ctx, aura.target_guid);
        }
        crate::native::aura_changed(ctx, previous.as_ref(), Some(&aura));
        aura
    }

    pub(crate) fn delete(ctx: &ReducerContext, id: u64) -> bool {
        let auras = ctx.db.game_aura();
        let previous = auras.id().find(id);
        let removed = auras.id().delete(id);
        if let Some(aura) = previous {
            if aura.eff_kind == lyracore_shared::spell::taxonomy::A_STEALTH {
                crate::aoi::refresh_entity(ctx, aura.target_guid);
            }
            crate::native::aura_changed(ctx, Some(&aura), None);
        }
        removed
    }
}

crate::character_owned!(delete, fn sweep_delete_game_aura(ctx, character_guid) {
    let auras = ctx.db.game_aura();
    let ids: Vec<u64> = auras.by_target().filter(&character_guid).map(|a| a.id).collect();
    for id in ids {
        Aura::delete(ctx, id);
    }
});
crate::character_owned!(transfer, fn sweep_transfer_game_aura(ctx, character_guid, io) {
    table = game_aura,
    by = by_target,
    remint = id,
});

pub const CAST_FAIL_NONE: u8 = 0;
pub const CAST_FAIL_NO_POWER: u8 = 1;

// Codes are append-only for rolling Module and Gateway updates.
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
    // Client-initiated casts received synchronous packets, so relays must suppress duplicates.
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
    // A failed SMSG_CAST_RESULT releases the 1.12 client's queued melee ability.
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
}

pub const CAST_MISS_NONE: u8 = 0;
pub const CAST_MISS_MISS: u8 = 1;
/// Vanilla reports a magic spell's failed hit roll as a full resist.
pub const CAST_MISS_RESIST: u8 = 2;

impl SpellImpactEvent {
    pub(crate) fn insert(ctx: &ReducerContext, row: Self) -> Self {
        crate::native::spell_impact(ctx, &row);
        ctx.db.game_spell_impact_event_live().insert(row)
    }
}

impl SpellCastEvent {
    pub(crate) fn insert(ctx: &ReducerContext, row: Self) -> Self {
        crate::native::cast_event(ctx, &row);
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
            created_at: ctx.timestamp,
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
        }
    }
}

#[table(accessor = game_spell_cooldown, public)]
pub struct SpellCooldown {
    #[primary_key]
    pub caster_guid: u64,
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
}

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
        crate::aoi::dynamic_object_changed(ctx, &object);
    }

    #[cfg(feature = "debug_reducers")]
    pub(crate) fn repair_cell(mut self, ctx: &ReducerContext) {
        self.cell = self.position_cell();
        let object = ctx.db.game_dynamic_object().guid().update(self);
        crate::aoi::dynamic_object_changed(ctx, &object);
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
        crate::native::resurrect_offered(ctx, &row);
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
        crate::native::self_resurrection_changed(ctx, guid, spell_id.unwrap_or(0));
    }
}
