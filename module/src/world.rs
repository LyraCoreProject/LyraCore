use lyracore_shared::constants::sheath_state;
use lyracore_shared::spatial;
use spacetimedb::{reducer, table, Identity, ReducerContext, Table};

use crate::faction::game_faction_template;
use crate::graveyard;
use crate::helpers::entity_by_owner;
use crate::{
    game_character, game_character_buyback, game_corpse, game_creature_spline, game_instance,
};

mod simulation;
pub(crate) use simulation::backfill_simulation;

#[table(accessor = game_world_state_name)]
pub struct WorldStateName {
    #[primary_key]
    pub variable_id: i32,
}

#[table(
    accessor = game_world_state,
    index(accessor = by_partition, btree(columns = [map_id, instance_id])),
    index(accessor = by_instance, btree(columns = [instance_id]))
)]
pub struct WorldState {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub map_id: u32,
    pub instance_id: u64,
    pub variable_id: i32,
    pub value: i32,
}

pub(crate) fn set_relay_world_state(
    ctx: &ReducerContext,
    source_guid: u64,
    variable_id: i32,
    value: i32,
) -> Result<(), String> {
    let source = ctx
        .db
        .game_world_entity()
        .guid()
        .find(source_guid)
        .ok_or_else(|| format!("world-state source {source_guid} is unavailable"))?;
    if ctx
        .db
        .game_world_state_name()
        .variable_id()
        .find(variable_id)
        .is_none()
    {
        return Err(format!("world-state variable {variable_id} is not named"));
    }
    let states = ctx.db.game_world_state();
    if let Some(mut row) = states
        .by_partition()
        .filter((source.map_id, source.instance_id))
        .find(|row| row.variable_id == variable_id)
    {
        row.value = value;
        states.id().update(row);
    } else {
        states.insert(WorldState {
            id: 0,
            map_id: source.map_id,
            instance_id: source.instance_id,
            variable_id,
            value,
        });
    }
    Ok(())
}

pub(crate) fn clear_relay_world_states_for_instance(ctx: &ReducerContext, instance_id: u64) {
    let states = ctx.db.game_world_state();
    for row in states
        .by_instance()
        .filter(&instance_id)
        .collect::<Vec<_>>()
    {
        states.id().delete(row.id);
    }
}

#[derive(PartialEq)]
#[table(
    accessor = game_world_entity,
    public,
    index(accessor = by_map, btree(columns = [map_id])),
    index(accessor = by_grid, btree(columns = [map_id, instance_id, grid_x, grid_y])),
    // SpacetimeDB 2.7.1 serves this subscription only with three equality-indexed columns.
    index(accessor = by_cell, btree(columns = [map_id, instance_id, cell])),
    index(accessor = by_cell_entry, btree(columns = [map_id, instance_id, cell, entry])),
    index(accessor = by_owner, btree(columns = [owner_identity])),
    index(accessor = by_entry, btree(columns = [entry])),
    index(accessor = by_owner_guid, btree(columns = [owner_guid])),
    index(accessor = by_regeneration_pending, btree(columns = [regeneration_pending])),
    index(accessor = by_combat_active, btree(columns = [combat_active])),
    index(accessor = by_simulation_indexed, btree(columns = [simulation_indexed]))
)]
pub struct WorldEntity {
    #[primary_key]
    pub guid: u64,
    pub owner_identity: Identity,
    pub account_id: u64,

    pub map_id: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub orientation: f32,
    pub grid_x: i32,
    pub grid_y: i32,
    pub last_move_ms: u32,

    pub type_mask: u32,
    pub entry: u32,
    pub scale_x: f32,

    pub health: u32,
    pub max_health: u32,
    pub power: u32,
    pub max_power: u32,
    pub level: u32,
    pub faction_template: u32,
    pub unit_bytes_0: u32,
    pub display_id: u32,
    pub native_display_id: u32,
    pub unit_flags: u32,
    pub base_attack_time_ms: u32,
    pub dynamic_flags: u32,
    pub dead: bool,

    pub player_bytes: u32,
    pub player_bytes_2: u32,
    pub player_bytes_3: u32,
    pub player_flags: u32,
    pub xp: u32,
    pub next_level_xp: u32,

    pub target_guid: u64,

    #[default(0)]
    pub money: u32,

    #[default(0)]
    pub unit_bytes_1: u32,

    #[default(0)]
    pub strength: u32,
    #[default(0)]
    pub agility: u32,
    #[default(0)]
    pub stamina: u32,
    #[default(0)]
    pub intellect: u32,
    #[default(0)]
    pub spirit: u32,

    #[default(0)]
    pub npc_flags: u32,

    #[default(0)]
    pub armor: u32,

    #[default(0)]
    pub leg_ends_ms: u32,
    #[default(0u64)]
    // Migration defaults must encode u64 as eight bytes; an untyped zero encodes four.
    pub wp_target: u64,
    #[default(0u32)]
    pub movement_flags: u32,
    #[default(0u64)]
    pub combat_until_ms: u64,
    #[default(false)]
    pub pickpocketed: bool,
    #[default(0)]
    pub next_swing_spell: u32,
    #[default(0u64)]
    pub overpower_until_ms: u64,
    #[default(0u64)]
    pub revenge_until_ms: u64,
    #[default(0)]
    pub stance: u8,
    #[default(0u64)]
    pub owner_guid: u64,
    #[default(false)]
    pub skinned: bool,
    #[default(0u64)]
    pub mana_regen_paused_until_ms: u64,
    #[default(0i64)]
    pub death_expire_micros: i64,

    #[default(0u64)]
    pub instance_id: u64,
    #[default(10000)]
    pub run_speed_mult_bp: u32,

    #[default(false)]
    pub godmode: bool,
    #[default(false)]
    pub resting: bool,
    #[default(0i64)]
    pub cell: i64,
    #[default(0)]
    pub sheet_str_bonus: i32,
    #[default(0)]
    pub sheet_agi_bonus: i32,
    #[default(0)]
    pub sheet_sta_bonus: i32,
    #[default(0)]
    pub sheet_int_bonus: i32,
    #[default(0)]
    pub sheet_spi_bonus: i32,
    #[default(0)]
    pub sheet_ap_base: u32,
    #[default(0)]
    pub sheet_ap_mods: i32,
    #[default(0)]
    pub sheet_dmg_min: u32,
    #[default(0)]
    pub sheet_dmg_max: u32,
    #[default(0)]
    pub sheet_crit_bp: u32,
    #[default(0)]
    pub unit_bytes_2: u32,
    #[default(0)]
    pub bank_bag_slots: u8,
    #[default(0)]
    pub mount_display_id: u32,
    #[default(0)]
    pub zone_id: u32,
    #[default(0)]
    pub sheet_ranged_ap: u32,
    #[default(0)]
    pub sheet_ranged_dmg_min: u32,
    #[default(0)]
    pub sheet_ranged_dmg_max: u32,
    #[default(false)]
    pub regeneration_pending: bool,
    #[default(false)]
    pub combat_active: bool,
    #[default(false)]
    pub simulation_indexed: bool,
}

impl WorldEntity {
    pub(crate) fn is_player(&self) -> bool {
        self.type_mask & lyracore_shared::constants::type_mask::PLAYER_BIT != 0
    }

    pub(crate) fn race(&self) -> u8 {
        (self.unit_bytes_0 & 0xFF) as u8
    }

    pub(crate) fn class(&self) -> u8 {
        ((self.unit_bytes_0 >> 8) & 0xFF) as u8
    }

    /// The pool `power` holds (`lyracore_shared::packing::power_type`) — byte 3 of `unit_bytes_0`.
    pub(crate) fn power_type(&self) -> u8 {
        ((self.unit_bytes_0 >> 24) & 0xFF) as u8
    }
}

#[table(
    accessor = game_entity_motion,
    public,
    index(accessor = by_grid, btree(columns = [map_id, instance_id, grid_x, grid_y])),
    index(accessor = by_cell, btree(columns = [map_id, instance_id, cell]))
)]
pub struct EntityMotion {
    #[primary_key]
    pub guid: u64,
    pub map_id: u32,
    pub instance_id: u64,
    pub grid_x: i32,
    pub grid_y: i32,
    pub opcode: u16,
    pub movement_info: Vec<u8>,
    pub seq: u32,
    #[default(0i64)]
    pub cell: i64,
}

pub const MOVE_VIOLATION_SPEED: u8 = 1;
pub const MOVE_VIOLATION_TELEPORT: u8 = 2;

const SPEED_LEEWAY: f32 = 1.8;
const TELEPORT_MAX_YD: f32 = 60.0;
const MIN_DT_S: f32 = 0.05;

pub(crate) fn movement_violation(dist_2d: f32, dt_s: f32, max_speed: f32) -> Option<(u8, f32)> {
    if dist_2d > TELEPORT_MAX_YD {
        return Some((MOVE_VIOLATION_TELEPORT, dist_2d));
    }
    if dt_s >= MIN_DT_S && max_speed > 0.0 {
        let observed = dist_2d / dt_s;
        if observed > max_speed * SPEED_LEEWAY {
            return Some((MOVE_VIOLATION_SPEED, observed / max_speed));
        }
    }
    None
}

fn is_gm_character(ctx: &ReducerContext, guid: u64) -> bool {
    ctx.db
        .game_character()
        .guid()
        .find(guid)
        .map(|c| c.gm_level != 0)
        .unwrap_or(false)
}

pub(crate) struct MovementDelta {
    pub old_x: f32,
    pub old_y: f32,
    pub old_z: f32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub old_move_ms: u32,
    pub move_time_ms: u32,
}

impl MovementDelta {
    pub(crate) fn moved(&self) -> bool {
        (self.x - self.old_x).powi(2)
            + (self.y - self.old_y).powi(2)
            + (self.z - self.old_z).powi(2)
            > 0.0001
    }
}

pub(crate) fn score_and_log_movement(ctx: &ReducerContext, guid: u64, delta: &MovementDelta) {
    let dist_2d = ((delta.x - delta.old_x).powi(2) + (delta.y - delta.old_y).powi(2)).sqrt();
    let dt_s = if delta.move_time_ms > delta.old_move_ms {
        (delta.move_time_ms - delta.old_move_ms) as f32 / 1000.0
    } else {
        0.0
    };
    if dist_2d <= TELEPORT_MAX_YD && dt_s < MIN_DT_S {
        return;
    }
    let max_speed =
        crate::combat::effective_move_speed(ctx, guid, lyracore_shared::constants::speeds::RUN);
    let Some((kind, magnitude)) = movement_violation(dist_2d, dt_s, max_speed) else {
        return;
    };
    match ctx.db.game_world_entity().guid().find(guid) {
        Some(e) if e.is_player() && !e.godmode => {}
        _ => return,
    }
    if is_gm_character(ctx, guid) {
        return;
    }
    ctx.db.game_movement_violation().insert(MovementViolation {
        id: 0,
        guid,
        kind,
        magnitude,
        x: delta.x,
        y: delta.y,
        z: delta.z,
        created_at: ctx.timestamp,
    });
}

#[table(accessor = game_movement_violation)]
pub struct MovementViolation {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub guid: u64,
    pub kind: u8,
    pub magnitude: f32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub created_at: spacetimedb::Timestamp,
}

#[table(accessor = game_teleport_event, public, index(accessor = by_recipient, btree(columns = [recipient_identity])))]
pub struct TeleportEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub recipient_identity: Identity,
    pub mover_guid: u64,
    pub map_id: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub orientation: f32,
    #[default(0u64)]
    pub created_micros: u64,
    // Entity presence can lag Transfer; the Relay must use this authoritative cross-map flag.
    #[default(false)]
    pub cross_map: bool,
}

pub(crate) fn is_cross_map_teleport(current_map_id: u32, target_map_id: u32) -> bool {
    current_map_id != target_map_id
}

pub(crate) fn zone_transition(current: u32, resolved: Option<u32>) -> Option<u32> {
    resolved.filter(|zone| *zone != current)
}

pub(crate) fn apply_zone_transition(
    ctx: &ReducerContext,
    entity: &mut WorldEntity,
    resolved: Option<u32>,
) {
    let Some(zone) = zone_transition(entity.zone_id, resolved) else {
        return;
    };
    entity.zone_id = zone;
    if let Some(mut character) = crate::helpers::character_by_guid(ctx, entity.guid) {
        character.zone_id = zone;
        ctx.db.game_character().guid().update(character);
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn teleport_player(
    ctx: &ReducerContext,
    player_guid: u64,
    map_id: u32,
    instance_id: u64,
    x: f32,
    y: f32,
    z: f32,
    o: f32,
) {
    if crate::taxi::movement_is_suppressed(ctx, player_guid) {
        return;
    }
    let entities = ctx.db.game_world_entity();
    let Some(e) = entities.guid().find(player_guid) else {
        return;
    };
    let recipient_identity = e.owner_identity;
    let cross_map = is_cross_map_teleport(e.map_id, map_id);
    let changes_partition = e.map_id != map_id || e.instance_id != instance_id;
    if changes_partition {
        crate::duel::interrupt_duel_for(ctx, player_guid);
    }
    // Drop queued pre-teleport motion so the next publish cannot snap peers back.
    crate::motion::drop_pending(ctx, player_guid);

    let destination_zone = crate::terrain::zone_id_at(ctx, map_id, x, y);

    if cross_map {
        persist_entity(ctx, &e, false);
        crate::combat::disengage(ctx, player_guid);
        crate::spell::break_stealth(ctx, player_guid);
        ctx.db.game_entity_motion().guid().delete(player_guid);
        entities.guid().delete(player_guid);
    } else {
        let (grid_x, grid_y) = spatial::grid_cell(x, y);
        let mut e = e;
        e.map_id = map_id;
        e.x = x;
        e.y = y;
        e.z = z;
        e.orientation = o;
        e.grid_x = grid_x;
        e.grid_y = grid_y;
        e.cell = lyracore_shared::spatial::grid_cell_id(grid_x, grid_y);
        e.instance_id = instance_id;
        if let Some(zone) = destination_zone {
            e.zone_id = zone;
        }
        e.update(ctx);
    }

    if !crate::mount::mount_cast_allowed_here(ctx, map_id, x, y, z) {
        crate::mount::dismount(ctx, player_guid);
    }

    let chars = ctx.db.game_character();
    if let Some(mut c) = chars.guid().find(player_guid) {
        c.map_id = map_id;
        c.x = x;
        c.y = y;
        c.z = z;
        c.orientation = o;
        c.pending_instance_id = instance_id;
        if let Some(zone) = destination_zone {
            c.zone_id = zone;
        }
        chars.guid().update(c);
    }

    if let Some(previous) = ctx.db.game_creature_spline().guid().find(player_guid) {
        let spline_id = crate::creatures::tick::next_spline_id(
            crate::creatures::tick::now_ms(ctx),
            previous.spline_id,
        );
        crate::creatures::tick::emit_move_spline(
            ctx,
            player_guid,
            (x, y, z),
            (x, y, z),
            0,
            false,
            spline_id,
            map_id,
            instance_id,
            spatial::grid_cell(x, y),
        );
    }

    crate::hooks::fire_on_character_relocated(
        ctx,
        &crate::hooks::CharacterRelocatedPayload {
            character_guid: player_guid,
        },
    );

    ctx.db.game_teleport_event().insert(TeleportEvent {
        id: 0,
        recipient_identity,
        mover_guid: player_guid,
        map_id,
        x,
        y,
        z,
        orientation: o,
        created_micros: ctx.timestamp.to_micros_since_unix_epoch() as u64,
        cross_map,
    });

    if changes_partition {
        crate::instance::reconcile_instance_removal(ctx, player_guid);
    }
}

pub(crate) fn set_home(ctx: &ReducerContext, guid: u64) {
    let Some(e) = ctx.db.game_world_entity().guid().find(guid) else {
        return;
    };
    let chars = ctx.db.game_character();
    if let Some(mut c) = chars.guid().find(guid) {
        c.home_map = e.map_id;
        c.home_x = e.x;
        c.home_y = e.y;
        c.home_z = e.z;
        c.home_zone = c.zone_id;
        chars.guid().update(c);
    }
}

pub(crate) fn recall_to_home(ctx: &ReducerContext, guid: u64) {
    let Some(mut c) = crate::helpers::character_by_guid(ctx, guid) else {
        return;
    };
    if ctx.db.game_world_entity().guid().find(guid).is_some() {
        teleport_player(
            ctx,
            guid,
            c.home_map,
            0,
            c.home_x,
            c.home_y,
            c.home_z,
            c.orientation,
        );
        return;
    }
    if crate::taxi::movement_is_suppressed(ctx, guid) {
        return;
    }
    c.map_id = c.home_map;
    c.x = c.home_x;
    c.y = c.home_y;
    c.z = c.home_z;
    c.zone_id = c.home_zone;
    c.pending_instance_id = 0;
    ctx.db.game_character().guid().update(c);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorldEntry {
    Login,
    WorldPort,
}

pub(crate) fn apply_player_login(
    ctx: &ReducerContext,
    account: &crate::Account,
    character_guid: u64,
    owner: spacetimedb::Identity,
    entry: WorldEntry,
) -> Result<(), String> {
    let chars = ctx.db.game_character();
    let mut character = chars
        .guid()
        .find(character_guid)
        .ok_or_else(|| format!("no such character: {character_guid}"))?;
    if character.account_id != account.id {
        return Err("character does not belong to caller".to_string());
    }

    if crate::transfer::is_in_transit(ctx, character_guid) {
        return Err(format!(
            "character {character_guid} is in transit between shards"
        ));
    }

    restamp_owned_data(ctx, character_guid, owner);

    let entities = ctx.db.game_world_entity();
    // Persist the previous live entity before replacement so ghost relog cannot lose progression.
    if let Some(stale) = entities.guid().find(character_guid) {
        persist_entity(ctx, &stale, false);
        entities.guid().delete(character_guid);
        character = chars.guid().find(character_guid).unwrap_or(character);
    }

    if character.pending_instance_id != 0
        && ctx
            .db
            .game_instance()
            .instance_id()
            .find(character.pending_instance_id)
            .is_none()
    {
        let reaped_id = character.pending_instance_id;
        match crate::instance::stranding_fallback(
            crate::instance::imported_entrance_fallback(ctx, character.map_id),
            crate::instance::is_dungeon_map(character.map_id),
        ) {
            crate::instance::StrandingFallback::Entrance(m, x, y, z, o) => {
                character.map_id = m;
                character.x = x;
                character.y = y;
                character.z = z;
                character.orientation = o;
            }
            crate::instance::StrandingFallback::InPlaceOpenWorld => {}
            crate::instance::StrandingFallback::HearthstoneHome => {
                spacetimedb::log::warn!(
                    "player_login: dungeon map {} has no imported entrance; moving {} to hearthstone",
                    character.map_id, character.guid
                );
                character.map_id = character.home_map;
                character.x = character.home_x;
                character.y = character.home_y;
                character.z = character.home_z;
                character.orientation = 0.0;
            }
        }
        character.pending_instance_id = 0;
        spacetimedb::log::info!(
            "player_login: instance {reaped_id} was reaped — {} falls back to map {} at instance 0",
            character.guid,
            character.map_id
        );
    }
    if !character.pending_ghost {
        ctx.db
            .game_corpse()
            .guid()
            .delete(crate::corpse::corpse_guid_for(character_guid));
    }

    let mut entity = crate::creatures::build_player_entity(ctx, &character, owner);
    if character.pending_ghost {
        let (dead, health, player_flags, unit_bytes_1) =
            ghost_restored_fields(entity.player_flags, entity.unit_bytes_1);
        entity.dead = dead;
        entity.health = health;
        entity.player_flags = player_flags;
        entity.unit_bytes_1 = unit_bytes_1;
    }
    crate::away::at_world_entry(ctx, &mut entity, entry);
    entity.insert(ctx);
    crate::ghost::restore(ctx, character_guid);
    crate::spell::recompute_display(ctx, character_guid);
    crate::taxi::arm_taxi_flight(ctx, character_guid);
    crate::mount::restore_mount_on_rebuild(ctx, character_guid);

    crate::items::grant_starter_item(ctx, character.guid, owner)?;
    crate::spell::spellbook::grant_createinfo_spells(
        ctx,
        character.guid,
        owner,
        character.race,
        character.class,
    );

    crate::skill::ensure_player_skills(
        ctx,
        character.guid,
        owner,
        character.level as u32,
        character.class,
    );

    crate::talent::apply_learned_talents(ctx, character.guid, owner, character.level as u32);

    crate::spell::apply_racial_passives(ctx, character.guid, character.race, character.level);

    crate::spell::recompute_vitals(ctx, character.guid);
    crate::spell::recompute_sheet(ctx, character.guid);

    let (rested, consumed) = crate::xp::accrue_rested_on_login(
        ctx.timestamp.to_micros_since_unix_epoch(),
        character.last_logout_micros,
        character.rested_xp,
        character.level as u32,
        character.resting,
    );
    character.rested_xp = rested;
    if consumed {
        character.last_logout_micros = 0;
    }
    if character.resting {
        character.rested_since_micros = ctx.timestamp.to_micros_since_unix_epoch() as u64;
    }

    character.session_start_micros = ctx.timestamp.to_micros_since_unix_epoch() as u64;

    character.owner_identity = owner;
    character.online = true;
    character.first_login = false;
    chars.guid().update(character);
    crate::instance::reconcile_instance_removal(ctx, character_guid);
    crate::hooks::fire_on_login(ctx, &crate::hooks::LoginPayload { character_guid });
    Ok(())
}

fn restamp_owned_data(ctx: &ReducerContext, character_guid: u64, identity: Identity) {
    for sweep in crate::CHARACTER_OWNED_RESTAMP_SWEEPS {
        sweep(ctx, character_guid, identity);
    }
}

pub(crate) fn cascade_delete_character(ctx: &ReducerContext, character_guid: u64) {
    // Preserve the issued GUID floor before deleting the last row that refers to it.
    crate::auth::bump_guid_high_water(ctx, character_guid);
    for sweep in crate::CHARACTER_OWNED_DELETE_SWEEPS {
        sweep(ctx, character_guid);
    }

    {
        use crate::combo::game_combo_point;
        let combos = ctx.db.game_combo_point();
        let stale: Vec<u64> = combos
            .iter()
            .filter(|c| c.owner_guid == character_guid || c.target_guid == character_guid)
            .map(|c| c.id)
            .collect();
        for id in stale {
            combos.id().delete(id);
        }
    }

    crate::combat::disengage(ctx, character_guid);
    ctx.db
        .game_corpse()
        .guid()
        .delete(crate::corpse::corpse_guid_for(character_guid));
    crate::motion::drop_pending(ctx, character_guid);
    ctx.db.game_entity_motion().guid().delete(character_guid);
    ctx.db.game_world_entity().guid().delete(character_guid);
    ctx.db.game_character().guid().delete(character_guid);
}

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_delete_character(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    if crate::helpers::character_by_guid(ctx, character_guid).is_none()
        && ctx
            .db
            .game_character()
            .guid()
            .find(character_guid)
            .is_some()
    {
        return Err("CHAR_IN_TRANSIT".to_string());
    }
    crate::mail_text::drop_character_letters(ctx, character_guid);
    cascade_delete_character(ctx, character_guid);
    Ok(())
}

static MOVEMENT_ENTRIES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(crate) const PERSIST_MAX_DRIFT_YD: f32 = 4.0;

pub(crate) fn snapshot_needs_persist(
    opcode: u16,
    grid_changed: bool,
    flags_changed: bool,
    state_changed: bool,
    drift_sq: f32,
) -> bool {
    opcode as u32 != lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT
        || grid_changed
        || flags_changed
        || state_changed
        || drift_sq > PERSIST_MAX_DRIFT_YD * PERSIST_MAX_DRIFT_YD
}

pub(crate) fn resolve_environmental_damage(dmg: u32, health: u32) -> (u32, bool) {
    if dmg == 0 {
        return (health, false);
    }
    if health <= dmg {
        (health, true)
    } else {
        (health - dmg, false)
    }
}

// Relay every accepted heartbeat even when the entity snapshot needs no durable update.
pub(crate) struct MovementPlan {
    pub persist_entity: bool,

    pub relay_motion: bool,
    pub fall_lethal: bool,
    pub moved: bool,
}

/// Forward, backward and strafe: the movement a root forbids.
const STEERING_FLAGS: u32 = 0xF;
/// Steering plus jumping and falling far: "any movement" for a break-on-move aura.
const MOVING_FLAGS: u32 = STEERING_FLAGS | 0x2000 | 0x4000;

/// MovementInfo starts with a little-endian u32 movement mask.
pub(crate) fn carries_movement_flags(movement_info: &[u8], mask: u32) -> bool {
    movement_info
        .get(0..4)
        .is_some_and(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) & mask != 0)
}

pub(crate) fn movement_is_accepted(
    last_move_ms: u32,
    move_time_ms: u32,
    x: f32,
    y: f32,
    z: f32,
    o: f32,
) -> bool {
    (move_time_ms == 0 || move_time_ms > last_move_ms)
        && x.is_finite()
        && y.is_finite()
        && z.is_finite()
        && o.is_finite()
}

pub(crate) fn plan_movement(
    opcode: u16,
    grid_changed: bool,
    flags_changed: bool,
    state_changed: bool,
    drift_sq: f32,
    fall_lethal: bool,
    delta: &MovementDelta,
) -> MovementPlan {
    MovementPlan {
        persist_entity: snapshot_needs_persist(
            opcode,
            grid_changed,
            flags_changed,
            state_changed,
            drift_sq,
        ),
        relay_motion: true,
        fall_lethal,
        moved: delta.moved(),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_movement_update(
    ctx: &ReducerContext,
    mut mover: WorldEntity,
    opcode: u16,
    movement_info: Vec<u8>,
    x: f32,
    y: f32,
    z: f32,
    o: f32,
    move_time_ms: u32,
) -> Result<(), String> {
    if crate::taxi::movement_is_suppressed(ctx, mover.guid) {
        return Ok(());
    }
    {
        let n = MOVEMENT_ENTRIES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if n.is_multiple_of(2000) {
            spacetimedb::log::info!("MOVEMENT_ENTRIES total={n}");
        }
    }
    if mover.dead && mover.player_flags & lyracore_shared::constants::player_flags::GHOST == 0 {
        return Ok(());
    }

    // Rooted landings still need fall damage; stop receipts and turning remain accepted.
    if carries_movement_flags(&movement_info, STEERING_FLAGS)
        && opcode as u32 != lyracore_shared::opcodes::movement::MSG_MOVE_FALL_LAND
        && crate::spell::is_movement_blocked(ctx, mover.guid)
    {
        return Ok(());
    }

    let breaks_on_move = carries_movement_flags(&movement_info, MOVING_FLAGS);

    if !movement_is_accepted(mover.last_move_ms, move_time_ms, x, y, z, o) {
        return Ok(());
    }

    let (old_x, old_y, old_z) = (mover.x, mover.y, mover.z);
    let (old_gx, old_gy) = (mover.grid_x, mover.grid_y);
    let old_move_ms = mover.last_move_ms;
    let (old_flags, old_resting, old_health) = (mover.movement_flags, mover.resting, mover.health);

    let (grid_x, grid_y) = spatial::grid_cell(x, y);
    mover.x = x;
    mover.y = y;
    mover.z = z;
    mover.orientation = o;
    mover.grid_x = grid_x;
    mover.grid_y = grid_y;
    mover.cell = lyracore_shared::spatial::grid_cell_id(grid_x, grid_y);
    mover.last_move_ms = move_time_ms;
    mover.movement_flags = movement_info
        .get(0..4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .unwrap_or(0);
    let mover_guid = mover.guid;
    let map_id = mover.map_id;
    let instance_id = mover.instance_id;
    let mut fall_lethal = false;
    if opcode as u32 == lyracore_shared::opcodes::movement::MSG_MOVE_FALL_LAND
        && !mover.dead
        && !mover.godmode
        && mover.is_player()
    {
        if let Some(ft) = lyracore_shared::env::fall_time_from_movement_info(&movement_info) {
            let dmg = lyracore_shared::env::fall_damage(ft, mover.max_health);
            let (health, lethal) = resolve_environmental_damage(dmg, mover.health);
            mover.health = health;
            fall_lethal = lethal;
        }
    }
    if mover.is_player() && !mover.dead && (grid_x != old_gx || grid_y != old_gy) {
        if let Some(area) = crate::terrain::area_at(ctx, mover.map_id, mover.x, mover.y) {
            crate::exploration::check_area_exploration(ctx, &mut mover, &area);
            apply_zone_transition(ctx, &mut mover, Some(crate::terrain::zone_of(&area)));
        }
    }
    if mover.is_player() && !mover.dead && (move_time_ms / 1000 != old_move_ms / 1000) {
        crate::rest::check_rest_state(ctx, &mut mover);
    }
    if mover.is_player()
        && !mover.dead
        && !mover.godmode
        && mover.player_flags & lyracore_shared::constants::player_flags::GHOST == 0
        && (move_time_ms / 1000 != old_move_ms / 1000)
    {
        let liquid_level = crate::terrain::liquid_level_at(ctx, mover.map_id, mover.x, mover.y);
        let submerged = lyracore_shared::env::is_submerged(
            mover.z,
            liquid_level.unwrap_or_default(),
            liquid_level.is_some(),
            mover.movement_flags,
        );
        crate::breath::update_breath_edge(ctx, &mover, submerged);
    }
    if crate::fatigue::feels_fatigue(&mover) && (move_time_ms / 1000 != old_move_ms / 1000) {
        let deep_water_level =
            crate::terrain::deep_water_level_at(ctx, mover.map_id, mover.x, mover.y);
        let in_deep_water =
            lyracore_shared::env::is_in_deep_water(mover.z, deep_water_level, mover.movement_flags);
        crate::fatigue::update_fatigue_edge(ctx, &mover, in_deep_water);
    }
    let mount_indoor_due = mover.is_player()
        && mover.mount_display_id != 0
        && crate::mount::indoor_check_is_due(old_move_ms, move_time_ms);
    let drift_sq = (x - old_x).powi(2) + (y - old_y).powi(2) + (z - old_z).powi(2);
    let delta = MovementDelta {
        old_x,
        old_y,
        old_z,
        x,
        y,
        z,
        old_move_ms,
        move_time_ms,
    };
    let plan = plan_movement(
        opcode,
        grid_x != old_gx || grid_y != old_gy,
        mover.movement_flags != old_flags,
        mover.resting != old_resting || mover.health != old_health,
        drift_sq,
        fall_lethal,
        &delta,
    );

    if plan.persist_entity {
        mover.update(ctx);
    }
    if plan.fall_lethal {
        crate::combat::kill_player(ctx, mover_guid, mover_guid);
    }

    // Dismount after the entity write so its projection cannot be overwritten by the old snapshot.
    if mount_indoor_due && !crate::mount::mount_cast_allowed_here(ctx, map_id, x, y, z) {
        crate::mount::dismount(ctx, mover_guid);
    }

    if plan.moved {
        crate::spell::break_channel(ctx, mover_guid);
    }
    // Use client flags: a late stop can change stored position without breaking Shadowmeld.
    if breaks_on_move {
        crate::spell::break_auras_on_move(ctx, mover_guid);
    }

    if plan.moved {
        score_and_log_movement(ctx, mover_guid, &delta);
    }

    if plan.relay_motion {
        crate::motion::queue_motion(
            ctx,
            mover_guid,
            map_id,
            instance_id,
            grid_x,
            grid_y,
            opcode,
            movement_info,
        );
    }

    Ok(())
}

pub(crate) fn apply_set_target(
    ctx: &ReducerContext,
    mut player: WorldEntity,
    target_guid: u64,
) -> Result<(), String> {
    player.target_guid = target_guid;
    player.update(ctx);
    Ok(())
}

const INSPECT_RANGE_SQ: f32 = 100.0;

pub(crate) fn can_inspect(
    target_is_player: bool,
    same_map: bool,
    dist_sq: f32,
    friendly: bool,
) -> Result<(), String> {
    if !target_is_player {
        return Err("inspect target is not a player".to_string());
    }
    if !same_map {
        return Err("inspect target on another map".to_string());
    }
    if dist_sq > INSPECT_RANGE_SQ {
        return Err("inspect target out of range".to_string());
    }
    if !friendly {
        return Err("inspect target is not friendly".to_string());
    }
    Ok(())
}

pub(crate) fn apply_inspect(
    ctx: &ReducerContext,
    inspector: crate::WorldEntity,
    target_guid: u64,
) -> Result<(), String> {
    let target = crate::helpers::live_entity(ctx, target_guid)
        .map_err(|_| "no such inspect target".to_string())?;
    let dist_sq = crate::helpers::dist_sq(&inspector, &target);
    let friendly = ctx.db.game_faction_template().count() == 0
        || crate::faction::is_friendly(ctx, inspector.faction_template, target.faction_template);
    can_inspect(
        target.is_player(),
        target.map_id == inspector.map_id && target.instance_id == inspector.instance_id,
        dist_sq,
        friendly,
    )
}

pub(crate) fn do_repop(ctx: &ReducerContext, guid: u64) -> Result<(), String> {
    use lyracore_shared::constants::{player_flags, unit_vis_flags};
    let entities = ctx.db.game_world_entity();
    let mut player = entities
        .guid()
        .find(guid)
        .ok_or_else(|| "caller not in world".to_string())?;
    if !player.dead {
        return Ok(());
    }
    if player.player_flags & player_flags::GHOST != 0 {
        return Ok(());
    }

    let (corpse_bytes_1, corpse_bytes_2) = crate::corpse::corpse_appearance_bytes(
        player.unit_bytes_0,
        player.player_bytes,
        player.player_bytes_2,
    );
    let now_micros = ctx.timestamp.to_micros_since_unix_epoch();
    let (new_expire, delay_micros) =
        crate::corpse::escalated_reclaim(player.death_expire_micros, now_micros);

    let corpse_guid = crate::corpse::corpse_guid_for(player.guid);
    let corpses = ctx.db.game_corpse();
    corpses.guid().delete(corpse_guid);
    corpses.insert(crate::Corpse {
        guid: corpse_guid,
        owner_guid: player.guid,
        map_id: player.map_id,
        x: player.x,
        y: player.y,
        z: player.z,
        orientation: player.orientation,
        display_id: player.native_display_id,
        bytes_1: corpse_bytes_1,
        bytes_2: corpse_bytes_2,
        created_at: ctx.timestamp,
        reclaim_delay_micros: delay_micros,
        is_bones: false,
        instance_id: player.instance_id,
    });

    player.health = 1;
    player.player_flags |= player_flags::GHOST;
    player.unit_bytes_1 |= unit_vis_flags::GHOST;
    player.death_expire_micros = new_expire;
    let player_guid = player.guid;
    let (death_x, death_y, death_map_id, death_race) =
        (player.x, player.y, player.map_id, player.race());
    player.update(ctx);
    crate::ghost::restore(ctx, player_guid);

    crate::creatures::despawn_pets(ctx, player_guid);

    let gy = graveyard::resolve_graveyard(ctx, death_map_id, death_x, death_y, death_race);
    teleport_player(ctx, player_guid, gy.map, 0, gy.x, gy.y, gy.z, gy.o);

    crate::spell::clear_resurrect_request(ctx, player_guid);
    Ok(())
}

pub(crate) fn persisted_pending_ghost(dead: bool, player_flags: u32) -> bool {
    dead && player_flags & lyracore_shared::constants::player_flags::GHOST != 0
}

pub(crate) const RUN_SPEED_BP_1X: u32 = 10_000;

pub(crate) fn persisted_gm_playtest(
    godmode: bool,
    run_speed_mult_bp: u32,
    set_offline: bool,
) -> (bool, u32) {
    if set_offline {
        (false, RUN_SPEED_BP_1X)
    } else {
        (godmode, run_speed_mult_bp)
    }
}

pub(crate) fn ghost_restored_fields(player_flags: u32, unit_bytes_1: u32) -> (bool, u32, u32, u32) {
    use lyracore_shared::constants::{player_flags as pf, unit_vis_flags};
    (
        true,
        1,
        player_flags | pf::GHOST,
        unit_bytes_1 | unit_vis_flags::GHOST,
    )
}

pub(crate) fn apply_set_sheathed(
    ctx: &ReducerContext,
    mut actor: WorldEntity,
    state: u8,
) -> Result<(), String> {
    if !sheath_state::is_valid(state) {
        return Err(format!("invalid sheath state {state}"));
    }
    let packed = sheath_state::packed_with(actor.unit_bytes_2, state);
    if packed == actor.unit_bytes_2 {
        return Ok(());
    }
    actor.unit_bytes_2 = packed;
    actor.update(ctx);
    Ok(())
}

// The states a client may pick: stand, sit, sleep, kneel.
const PLAYER_STAND_STATES: [u8; 4] = [0, 1, 3, 8];

const STAND_STATE_DEAD: u32 = 7;

// No alive check: the client sits a released ghost.
pub(crate) fn apply_set_stand_state(
    ctx: &ReducerContext,
    mut actor: WorldEntity,
    state: u8,
) -> Result<(), String> {
    if !PLAYER_STAND_STATES.contains(&state) {
        return Err(format!("invalid stand state {state}"));
    }
    let packed = (actor.unit_bytes_1 & !0xFF) | u32::from(state);
    if packed == actor.unit_bytes_1 {
        return Ok(());
    }
    actor.unit_bytes_1 = packed;
    actor.update(ctx);
    Ok(())
}

// The client does not stand itself when hit.
pub(crate) fn stand_up_on_damage(ctx: &ReducerContext, target_guid: u64) {
    let entities = ctx.db.game_world_entity();
    let Some(mut target) = entities.guid().find(target_guid) else {
        return;
    };
    let state = target.unit_bytes_1 & 0xFF;
    if !target.is_player()
        || target.mount_display_id != 0
        || state == 0
        || state == STAND_STATE_DEAD
    {
        return;
    }
    target.unit_bytes_1 &= !0xFF;
    target.update(ctx);
}

pub const RESURRECTION_SICKNESS_SPELL: u32 = 15007;

fn spirit_res_vitals(max_health: u32, max_power: u32) -> (u32, u32) {
    ((max_health / 2).max(1), max_power / 2)
}

pub(crate) fn do_spirit_healer_res(ctx: &ReducerContext, guid: u64) -> Result<(), String> {
    use lyracore_shared::constants::player_flags;
    let entities = ctx.db.game_world_entity();
    let mut player = entities
        .guid()
        .find(guid)
        .ok_or_else(|| "caller not in world".to_string())?;
    if !player.dead || player.player_flags & player_flags::GHOST == 0 {
        return Err("caller is not a ghost".to_string());
    }
    let (health, power) = spirit_res_vitals(player.max_health, player.max_power);
    let player_level = player.level;
    player.health = health;
    player.power = power;
    player.update(ctx);
    crate::ghost::finish_resurrection(ctx, guid);
    if player_level >= 11 {
        crate::spell::apply_spell_auras(ctx, RESURRECTION_SICKNESS_SPELL, guid, 1, 1);
    }
    Ok(())
}

fn accrue_played_on_persist(now_micros: i64, session_start_micros: u64, pool: u32) -> u32 {
    if session_start_micros == 0 {
        return pool;
    }
    let elapsed_secs = (now_micros as u64).saturating_sub(session_start_micros) / 1_000_000;
    pool.saturating_add(elapsed_secs as u32)
}

pub(crate) fn persist_entity(ctx: &ReducerContext, entity: &WorldEntity, set_offline: bool) {
    let chars = ctx.db.game_character();
    if let Some(mut c) = chars.guid().find(entity.guid) {
        c.x = entity.x;
        c.y = entity.y;
        c.z = entity.z;
        c.orientation = entity.orientation;
        if let Some(z) = crate::terrain::zone_id_at(ctx, entity.map_id, entity.x, entity.y) {
            c.zone_id = z;
        }
        c.level = entity.level as u8;
        c.xp = entity.xp;
        c.next_level_xp = entity.next_level_xp;
        c.money = entity.money;
        c.bank_bag_slots = entity.bank_bag_slots;
        // Zero means uninitialized health; pending_ghost restores a released ghost separately.
        c.health = entity.health.max(1);
        c.power = entity.power;

        c.death_expire_micros = entity.death_expire_micros;

        c.pending_instance_id = entity.instance_id;
        c.pending_ghost = persisted_pending_ghost(entity.dead, entity.player_flags);
        let (pending_godmode, pending_speed_bp) =
            persisted_gm_playtest(entity.godmode, entity.run_speed_mult_bp, set_offline);
        c.pending_godmode = pending_godmode;
        c.pending_run_speed_mult_bp = pending_speed_bp;
        // Close the played-time span on every persist so relog cannot count it twice.
        c.played_total_secs = accrue_played_on_persist(
            ctx.timestamp.to_micros_since_unix_epoch(),
            c.session_start_micros,
            c.played_total_secs,
        );
        c.session_start_micros = 0;
        if set_offline {
            c.online = false;
            c.last_logout_micros = ctx.timestamp.to_micros_since_unix_epoch() as u64;
            c.resting = entity.resting;
            crate::rest::materialize_on_logout(&mut c, ctx.timestamp.to_micros_since_unix_epoch());
        }
        chars.guid().update(c);
    }
}

pub(crate) fn remove_from_world(ctx: &ReducerContext, owner: Identity) {
    let Some(entity) = entity_by_owner(ctx, owner) else {
        return;
    };
    remove_live_character(ctx, entity);
}

pub(crate) fn remove_live_character(ctx: &ReducerContext, entity: WorldEntity) {
    // Fire logout hooks before deletion while handlers can still read the live entity.
    crate::hooks::fire_on_logout(
        ctx,
        &crate::hooks::LogoutPayload {
            character_guid: entity.guid,
        },
    );
    crate::duel::interrupt_duel_for(ctx, entity.guid);
    persist_entity(ctx, &entity, true);

    crate::combat::disengage(ctx, entity.guid);

    crate::trade::cancel_trade_for(ctx, entity.guid);

    if !persisted_pending_ghost(entity.dead, entity.player_flags) {
        ctx.db
            .game_corpse()
            .guid()
            .delete(crate::corpse::corpse_guid_for(entity.guid));
    }

    crate::spell::clear_resurrect_request_and_option(ctx, entity.guid);

    crate::spell::break_stealth(ctx, entity.guid);

    crate::creatures::despawn_pets(ctx, entity.guid);

    for row in ctx
        .db
        .game_character_buyback()
        .by_player_guid()
        .filter(&entity.guid)
        .collect::<Vec<_>>()
    {
        ctx.db.game_character_buyback().id().delete(row.id);
    }

    crate::motion::drop_pending(ctx, entity.guid);
    ctx.db.game_entity_motion().guid().delete(entity.guid);
    ctx.db.game_world_entity().guid().delete(entity.guid);
}

pub(crate) fn apply_gossip_select(
    ctx: &ReducerContext,
    character_guid: u64,
    npc_guid: u64,
    option_id: u32,
    option_row_id: u32,
    source_menu_id: u32,
    selected_action: u32,
) -> Result<(), String> {
    crate::gossip::select_gate(
        ctx,
        character_guid,
        npc_guid,
        option_row_id,
        source_menu_id,
        selected_action,
    )?;
    crate::hooks::fire_on_gossip_select(
        ctx,
        &crate::hooks::GossipSelectPayload {
            character_guid,
            npc_guid,
            option_id,
            option_row_id,
        },
    );
    Ok(())
}

#[reducer(client_disconnected)]
pub fn on_disconnect(ctx: &ReducerContext) {
    remove_from_world(ctx, ctx.sender());
}

#[cfg(test)]
mod tests {
    use super::{
        accrue_played_on_persist, can_inspect, ghost_restored_fields, is_cross_map_teleport,
        movement_violation, persisted_gm_playtest, persisted_pending_ghost, plan_movement,
        resolve_environmental_damage, snapshot_needs_persist, spirit_res_vitals, zone_transition,
        MovementDelta, INSPECT_RANGE_SQ, MOVE_VIOLATION_SPEED, MOVE_VIOLATION_TELEPORT,
        PERSIST_MAX_DRIFT_YD, RESURRECTION_SICKNESS_SPELL, RUN_SPEED_BP_1X,
    };

    const HEARTBEAT: u16 = lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT as u16;

    #[test]
    fn a_heartbeat_that_changed_nothing_observable_is_not_persisted() {
        assert!(!snapshot_needs_persist(HEARTBEAT, false, false, false, 0.0));
        assert!(!snapshot_needs_persist(
            HEARTBEAT,
            false,
            false,
            false,
            3.9 * 3.9
        ));
    }

    #[test]
    fn every_observable_change_forces_the_row_write() {
        assert!(
            snapshot_needs_persist(HEARTBEAT, true, false, false, 0.0),
            "grid cell (AOI)"
        );
        assert!(
            snapshot_needs_persist(HEARTBEAT, false, true, false, 0.0),
            "movement flags"
        );
        assert!(
            snapshot_needs_persist(HEARTBEAT, false, false, true, 0.0),
            "resting/health"
        );
        let facing = lyracore_shared::opcodes::movement::MSG_MOVE_SET_FACING as u16;
        assert!(
            snapshot_needs_persist(facing, false, false, false, 0.0),
            "non-heartbeat opcode"
        );
    }

    #[test]
    fn drift_past_the_bound_forces_the_row_write() {
        let at = PERSIST_MAX_DRIFT_YD * PERSIST_MAX_DRIFT_YD;
        assert!(!snapshot_needs_persist(HEARTBEAT, false, false, false, at));
        assert!(snapshot_needs_persist(
            HEARTBEAT,
            false,
            false,
            false,
            at + 0.01
        ));
        const {
            assert!(
                PERSIST_MAX_DRIFT_YD <= 5.0,
                "drift must stay inside melee reach (5 yd)"
            )
        };
    }

    #[test]
    fn resolve_environmental_damage_subtracts_short_of_lethal_and_flags_lethal_without_mutating() {
        assert_eq!(
            resolve_environmental_damage(0, 50),
            (50, false),
            "a soft landing (no damage) is a no-op"
        );
        assert_eq!(
            resolve_environmental_damage(20, 50),
            (30, false),
            "sub-lethal damage subtracts"
        );
        assert_eq!(
            resolve_environmental_damage(50, 50),
            (50, true),
            "exactly-lethal damage is flagged, not subtracted — kill_player re-fetches fresh"
        );
        assert_eq!(
            resolve_environmental_damage(80, 50),
            (50, true),
            "over-lethal damage is flagged, not subtracted either"
        );
    }

    #[test]
    fn a_zone_transition_writes_only_when_the_resolved_zone_is_new() {
        assert_eq!(zone_transition(12, Some(40)), Some(40));
        assert_eq!(zone_transition(0, Some(12)), Some(12));
        assert_eq!(zone_transition(12, Some(12)), None);
        assert_eq!(zone_transition(12, None), None);
        assert_eq!(zone_transition(0, None), None);
    }

    #[test]
    fn plan_movement_mirrors_snapshot_needs_persist_and_never_gates_the_relay() {
        let stationary = MovementDelta {
            old_x: 0.0,
            old_y: 0.0,
            old_z: 0.0,
            x: 0.0,
            y: 0.0,
            z: 0.0,
            old_move_ms: 0,
            move_time_ms: 0,
        };
        for &(grid, flags, state, drift) in &[
            (false, false, false, 0.0),
            (true, false, false, 0.0),
            (false, true, false, 0.0),
            (false, false, true, 0.0),
            (
                false,
                false,
                false,
                PERSIST_MAX_DRIFT_YD * PERSIST_MAX_DRIFT_YD + 1.0,
            ),
        ] {
            let want = snapshot_needs_persist(HEARTBEAT, grid, flags, state, drift);
            let plan = plan_movement(HEARTBEAT, grid, flags, state, drift, false, &stationary);
            assert_eq!(
                plan.persist_entity, want,
                "persist_entity must mirror snapshot_needs_persist exactly"
            );
            assert!(
                plan.relay_motion,
                "the motion relay must never be gated on the persist decision"
            );
        }
        let lethal = plan_movement(HEARTBEAT, false, false, false, 0.0, true, &stationary);
        assert!(lethal.fall_lethal);
        assert!(!lethal.persist_entity);
        assert!(
            lethal.relay_motion,
            "even a lethal-fall heartbeat still relays"
        );
        let moving = MovementDelta {
            old_x: 0.0,
            old_y: 0.0,
            old_z: 0.0,
            x: 10.0,
            y: 0.0,
            z: 0.0,
            old_move_ms: 0,
            move_time_ms: 500,
        };
        assert!(plan_movement(HEARTBEAT, false, false, false, 0.0, false, &moving).moved);
        assert!(!plan_movement(HEARTBEAT, false, false, false, 0.0, false, &stationary).moved);
    }

    #[test]
    fn movement_violation_flags_speed_and_teleport_but_not_legit_motion() {
        let run = lyracore_shared::constants::speeds::RUN;
        assert_eq!(movement_violation(3.5, 0.5, run), None);
        assert_eq!(movement_violation(run * 1.5 * 0.5, 0.5, run), None);
        let (k, mag) = movement_violation(run * 3.0 * 0.5, 0.5, run).unwrap();
        assert_eq!(k, MOVE_VIOLATION_SPEED);
        assert!(
            (mag - 3.0).abs() < 1e-3,
            "magnitude is the ×-normal multiple"
        );
        let (k, mag) = movement_violation(100.0, 0.0, run).unwrap();
        assert_eq!(k, MOVE_VIOLATION_TELEPORT);
        assert_eq!(mag, 100.0);
        assert_eq!(movement_violation(5.0, 0.01, run), None);
        assert!(
            movement_violation(run * 2.0 * 0.5, 0.5, run / 2.0).is_some(),
            "snared mover flagged sooner"
        );
    }

    #[test]
    fn is_cross_map_teleport_only_when_target_map_differs() {
        assert!(
            !is_cross_map_teleport(0, 0),
            "same map (both open world) is a same-map hop"
        );
        assert!(
            !is_cross_map_teleport(36, 36),
            "same map (both inside the same instance map) is a same-map hop"
        );
        assert!(
            is_cross_map_teleport(0, 36),
            "Elwynn -> Deadmines is cross-map"
        );
        assert!(
            is_cross_map_teleport(36, 0),
            "Deadmines -> Elwynn (exit) is cross-map"
        );
    }

    #[test]
    fn pending_ghost_preserves_only_released_ghosts() {
        use lyracore_shared::constants::player_flags::GHOST;
        assert!(persisted_pending_ghost(true, GHOST));
        assert!(!persisted_pending_ghost(false, GHOST));
        assert!(!persisted_pending_ghost(true, 0));
    }

    #[test]
    fn gm_playtest_state_carries_across_a_map_change_and_resets_on_a_real_logout() {
        assert_eq!(
            persisted_gm_playtest(true, 30_000, false),
            (true, 30_000),
            "a cross-map hop carries godmode and the speed multiplier"
        );
        assert_eq!(
            persisted_gm_playtest(true, 30_000, true),
            (false, RUN_SPEED_BP_1X),
            "a real logout resets GM playtest state to off / 1x"
        );
        assert_eq!(
            persisted_gm_playtest(false, RUN_SPEED_BP_1X, false),
            (false, RUN_SPEED_BP_1X)
        );
    }

    #[test]
    fn ghost_restored_fields_reapply_exactly_the_release_state_over_the_fresh_build() {
        use lyracore_shared::constants::{player_flags, unit_vis_flags};
        let (dead, health, pf, ub1) = ghost_restored_fields(0, 17u32 << 16);
        assert!(dead, "a preserved ghost rebuilds dead");
        assert_eq!(
            health, 1,
            "ghost HP is 1 (vanilla), matching do_repop's release"
        );
        assert_eq!(pf, player_flags::GHOST, "the GHOST player flag is set");
        assert_eq!(
            ub1,
            (17u32 << 16) | unit_vis_flags::GHOST,
            "the ghost render bit ORs over the stance byte"
        );
        let (_, _, pf2, ub2) = ghost_restored_fields(pf, ub1);
        assert_eq!((pf2, ub2), (pf, ub1));
    }

    #[test]
    fn inspect_gate_admits_only_an_in_range_friendly_player() {
        assert!(can_inspect(true, true, 0.0, true).is_ok());
        assert!(can_inspect(true, true, INSPECT_RANGE_SQ, true).is_ok());

        assert_eq!(
            can_inspect(false, true, 0.0, true).unwrap_err(),
            "inspect target is not a player"
        );
        assert_eq!(
            can_inspect(true, false, 0.0, true).unwrap_err(),
            "inspect target on another map"
        );
        assert_eq!(
            can_inspect(true, true, INSPECT_RANGE_SQ + 1.0, true).unwrap_err(),
            "inspect target out of range"
        );
        assert_eq!(
            can_inspect(true, true, 0.0, false).unwrap_err(),
            "inspect target is not friendly"
        );
    }

    #[test]
    fn accrue_played_folds_elapsed_whole_seconds_into_the_pool() {
        assert_eq!(accrue_played_on_persist(5_000_000, 0, 42), 42);
        let now = 1_000_090_500_000_i64;
        let session_start = 1_000_000_000_000_u64;
        assert_eq!(accrue_played_on_persist(now, session_start, 10), 100);
        assert_eq!(accrue_played_on_persist(1_000_000_500, 1_000_000_000, 0), 0);
    }

    #[test]
    fn spirit_healer_res_restores_half_vitals_and_seeds_the_right_sickness() {
        assert_eq!(spirit_res_vitals(200, 80), (100, 40));
        assert_eq!(spirit_res_vitals(199, 81), (99, 40));
        assert_eq!(spirit_res_vitals(1, 0), (1, 0));
        assert_eq!(
            spirit_res_vitals(0, 0),
            (1, 0),
            "the .max(1) floor keeps a 0-max res alive"
        );
        assert_eq!(RESURRECTION_SICKNESS_SPELL, 15007);
    }
}
