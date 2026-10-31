use lyracore_protocol::codec::ClientMovement;
use lyracore_shared::constants::sheath_state;
use lyracore_shared::{movement_flags, spatial};
use spacetimedb::{reducer, table, Identity, ReducerContext, Table};

use crate::faction::game_faction_template;
use crate::graveyard;
use crate::helpers::entity_by_owner;
use crate::{
    game_aura, game_character, game_character_buyback, game_creature_spline, game_instance,
};

mod placement;
mod simulation;

pub(crate) use placement::MoveClock;
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
        .is_some_and(|c| crate::auth::gm_level(ctx, c.account_id) != 0)
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
    let dt_s = (delta.move_time_ms.wrapping_sub(delta.old_move_ms) as i32).max(0) as f32 / 1000.0;
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
        created_at: crate::time::now(ctx),
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
    teleport(ctx, player_guid, map_id, instance_id, [x, y, z, o], None);
}

/// Carries a Passenger to `map_id` with its Transport; `pose` is its world pose there.
pub(crate) fn teleport_aboard(
    ctx: &ReducerContext,
    map_id: u32,
    pose: [f32; 4],
    aboard: crate::transport::Aboard,
) {
    let guid = aboard.passenger.character_guid;
    teleport(ctx, guid, map_id, 0, pose, Some(aboard));
}

fn teleport(
    ctx: &ReducerContext,
    player_guid: u64,
    map_id: u32,
    instance_id: u64,
    [x, y, z, o]: [f32; 4],
    aboard: Option<crate::transport::Aboard>,
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
    let sessionless = crate::helpers::character_by_guid(ctx, player_guid)
        .is_some_and(|character| !character.online);
    let changes_partition = e.map_id != map_id || e.instance_id != instance_id;
    if changes_partition {
        crate::duel::interrupt_duel_for(ctx, player_guid);
        crate::trade::cancel_trade_for(ctx, player_guid);
    }
    let teleport_counter = crate::native::begin_teleport(ctx, player_guid, cross_map);
    // Drop queued pre-teleport motion so the next publish cannot snap peers back.
    crate::motion::drop_pending(ctx, player_guid);
    crate::transport::leave(ctx, player_guid);

    let destination_zone = crate::terrain::zone_id_at(ctx, map_id, x, y);

    if cross_map {
        persist_entity(ctx, &e, false);
        crate::combat::disengage(ctx, player_guid);
        crate::spell::break_stealth(ctx, player_guid);
        ctx.db.game_entity_motion().guid().delete(player_guid);
    }
    if cross_map && !sessionless {
        crate::WorldEntity::delete(ctx, player_guid);
    } else {
        let Some(mut e) = entities.guid().find(player_guid) else {
            return;
        };
        e.map_id = map_id;
        // A teleport takes the destination zone below; it explores nothing.
        e.place(x, y, z, MoveClock::Unchanged).unexplored();
        e.orientation = o;
        e.movement_flags = 0;
        e.instance_id = instance_id;
        if let Some(zone) = destination_zone {
            e.zone_id = zone;
        }
        if let Some(aboard) = &aboard {
            crate::transport::reboard(ctx, aboard.passenger.clone());
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
            None,
        );
    }

    crate::hooks::fire_on_character_relocated(
        ctx,
        &crate::hooks::CharacterRelocatedPayload {
            character_guid: player_guid,
        },
    );

    let event = ctx.db.game_teleport_event().insert(TeleportEvent {
        id: 0,
        recipient_identity,
        mover_guid: player_guid,
        map_id,
        x,
        y,
        z,
        orientation: o,
        created_micros: crate::time::now(ctx).to_micros_since_unix_epoch() as u64,
        cross_map,
    });
    crate::native::teleport_changed(ctx, &event, teleport_counter, aboard.as_ref());
    if let Some(aboard) = aboard.filter(|_| cross_map && !sessionless) {
        crate::transport::reboard(ctx, aboard.passenger);
    }

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
        crate::WorldEntity::delete(ctx, character_guid);
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
        crate::Corpse::delete(ctx, crate::corpse::corpse_guid_for(character_guid));
    }

    crate::taxi::sync_flight_on_world_entry(ctx, &mut character);

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
    crate::transport::arm_crossings(ctx);
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
        crate::time::now(ctx).to_micros_since_unix_epoch(),
        character.last_logout_micros,
        character.rested_xp,
        character.level as u32,
        character.resting,
    );
    character.rested_xp = rested;
    let previous_rest_state = match entry {
        WorldEntry::Login => lyracore_shared::packing::REST_STATE_NORMAL,
        WorldEntry::WorldPort => character.rest_state,
    };
    character.rest_state = crate::rest::rest_state_after_bonus(
        previous_rest_state,
        character.rested_xp,
        character.level,
    );
    if consumed {
        character.last_logout_micros = 0;
    }
    if character.resting {
        character.rested_since_micros = crate::time::now(ctx).to_micros_since_unix_epoch() as u64;
    }

    character.session_start_micros = crate::time::now(ctx).to_micros_since_unix_epoch() as u64;

    character.owner_identity = owner;
    character.online = true;
    character.first_login = false;
    let rest_state = character.rest_state;
    chars.guid().update(character);
    if let Some(mut entity) = ctx.db.game_world_entity().guid().find(character_guid) {
        let previous_bytes = entity.player_bytes_2;
        crate::rest::set_rest_state_byte(&mut entity, rest_state);
        let resolved_area = crate::terrain::area_at(ctx, entity.map_id, entity.x, entity.y);
        let resolved_zone = resolved_area.as_ref().map(crate::terrain::zone_of);
        let previous_zone = entity.zone_id;
        let was_resting = entity.resting;
        let previous_flags = entity.player_flags;
        if resolved_zone.is_some() {
            apply_zone_transition(ctx, &mut entity, resolved_zone);
            crate::rest::check_rest_state(ctx, &mut entity, resolved_area.as_ref());
        }
        if previous_zone != entity.zone_id
            || was_resting != entity.resting
            || previous_flags != entity.player_flags
            || previous_bytes != entity.player_bytes_2
        {
            entity.update(ctx);
        }
    }
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
    crate::guild::forget_deleted_character(ctx, character_guid);
    for sweep in crate::CHARACTER_OWNED_DELETE_SWEEPS {
        sweep(ctx, character_guid);
    }

    crate::combat::disengage(ctx, character_guid);
    crate::Corpse::delete(ctx, crate::corpse::corpse_guid_for(character_guid));
    crate::motion::drop_pending(ctx, character_guid);
    ctx.db.game_entity_motion().guid().delete(character_guid);
    crate::WorldEntity::delete(ctx, character_guid);
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

// Temporary profiling: time the steps of one received packet in 64.
static RX_PACKETS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static RX_SAMPLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn sample_packet() {
    let n = RX_PACKETS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    RX_SAMPLED.store(n % 64 == 0, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn span(name: &'static str) -> Option<crate::creatures::PassTimer> {
    RX_SAMPLED
        .load(std::sync::atomic::Ordering::Relaxed)
        .then(|| crate::creatures::PassTimer::start(name))
}

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

pub(crate) fn movement_is_accepted(
    last_move_ms: u32,
    move_time_ms: u32,
    x: f32,
    y: f32,
    z: f32,
    o: f32,
) -> bool {
    (move_time_ms == 0 || last_move_ms == 0 || (move_time_ms.wrapping_sub(last_move_ms) as i32) > 0)
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

/// `relay_body` is the MovementInfo the motion Relay sends to observers unchanged.
pub(crate) fn apply_movement_update(
    ctx: &ReducerContext,
    mut mover: WorldEntity,
    opcode: u16,
    movement: &ClientMovement,
    relay_body: Vec<u8>,
) -> Result<(), String> {
    let _total = span("mv.total");
    let gates = span("mv.gates");
    let info = movement.info();
    let (x, y, z) = (info.position.x, info.position.y, info.position.z);
    let (o, move_time_ms) = (info.orientation, info.timestamp);
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
    if movement.carries(movement_flags::STEERING)
        && opcode as u32 != lyracore_shared::opcodes::movement::MSG_MOVE_FALL_LAND
        && crate::spell::is_movement_blocked(ctx, mover.guid)
    {
        return Ok(());
    }

    let breaks_on_move = movement.carries(movement_flags::MOVING);

    if !movement_is_accepted(mover.last_move_ms, move_time_ms, x, y, z, o) {
        return Ok(());
    }

    drop(gates);
    let place = span("mv.place");
    let (old_x, old_y, old_z) = (mover.x, mover.y, mover.z);
    let (old_gx, old_gy) = (mover.grid_x, mover.grid_y);
    let old_move_ms = mover.last_move_ms;
    let (old_flags, old_resting, old_health) = (mover.movement_flags, mover.resting, mover.health);

    let crossing = mover.place(x, y, z, MoveClock::At(move_time_ms));
    let (grid_x, grid_y) = (mover.grid_x, mover.grid_y);
    mover.orientation = o;
    mover.movement_flags = movement.flags();
    let boarding = crate::transport::passenger_moved(ctx, &mover, old_flags, movement.transport());
    if boarding == crate::transport::Boarding::Refused {
        return Ok(());
    }
    let mover_guid = mover.guid;
    let map_id = mover.map_id;
    let instance_id = mover.instance_id;
    let mut fall_lethal = false;
    if opcode as u32 == lyracore_shared::opcodes::movement::MSG_MOVE_FALL_LAND
        && !mover.dead
        && !mover.godmode
        && mover.is_player()
        && !movement.carries(movement_flags::ON_TRANSPORT)
    {
        let dmg = lyracore_shared::env::fall_damage(movement.fall_time_ms(), mover.max_health);
        let (health, lethal) = resolve_environmental_damage(dmg, mover.health);
        mover.health = health;
        fall_lethal = lethal;
        crate::native::fall_damage(ctx, mover.guid, dmg);
    }
    drop(place);
    let explore = span("mv.explore");
    crossing.explore(ctx, &mut mover);
    drop(explore);
    let env = span("mv.env");
    if mover.is_player() && !mover.dead && (move_time_ms / 1000 != old_move_ms / 1000) {
        let area = crate::terrain::area_at(ctx, mover.map_id, mover.x, mover.y);
        crate::rest::check_rest_state(ctx, &mut mover, area.as_ref());
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
    drop(env);
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

    let persist = span("mv.persist");
    if plan.persist_entity {
        mover.update(ctx);
    }
    drop(persist);
    if plan.fall_lethal {
        crate::combat::kill_player(ctx, mover_guid, mover_guid);
    }

    // Dismount after the entity write so its projection cannot be overwritten by the old snapshot.
    if mount_indoor_due && !crate::mount::mount_cast_allowed_here(ctx, map_id, x, y, z) {
        crate::mount::dismount(ctx, mover_guid);
    }

    let breaks = span("mv.breaks");
    if plan.moved {
        crate::spell::break_channel(ctx, mover_guid);
    }
    // Use client flags: a late stop can change stored position without breaking Shadowmeld.
    if breaks_on_move {
        crate::spell::break_auras_on_move(ctx, mover_guid);
    }

    drop(breaks);
    let score = span("mv.score");
    if plan.moved && boarding == crate::transport::Boarding::Ashore {
        score_and_log_movement(ctx, mover_guid, &delta);
    }
    drop(score);
    let _relay = span("mv.relay");

    if plan.relay_motion {
        crate::motion::queue_motion(
            ctx,
            mover_guid,
            map_id,
            instance_id,
            grid_x,
            grid_y,
            opcode,
            relay_body,
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
) -> Result<(), crate::Refusal> {
    let target = crate::helpers::live_entity(ctx, target_guid)
        .map_err(|_| "no such inspect target".to_string())?;
    let dist_sq = crate::helpers::dist_sq(&inspector, &target);
    let friendly = ctx.db.game_faction_template().count() == 0
        || crate::faction::is_friendly(ctx, inspector.faction_template, target.faction_template);
    Ok(can_inspect(
        target.is_player(),
        target.map_id == inspector.map_id && target.instance_id == inspector.instance_id,
        dist_sq,
        friendly,
    )?)
}

pub(crate) fn do_repop(ctx: &ReducerContext, guid: u64) -> Result<(), crate::Refusal> {
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
    let now_micros = crate::time::now(ctx).to_micros_since_unix_epoch();
    let (new_expire, delay_micros) =
        crate::corpse::escalated_reclaim(player.death_expire_micros, now_micros);

    let corpse_guid = crate::corpse::corpse_guid_for(player.guid);
    crate::Corpse::delete(ctx, corpse_guid);
    crate::Corpse {
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
        created_at: crate::time::now(ctx),
        reclaim_delay_micros: delay_micros,
        is_bones: false,
        instance_id: player.instance_id,
        cell: 0,
    }
    .insert(ctx);

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
) -> Result<(), crate::Refusal> {
    if !sheath_state::is_valid(state) {
        return Err(format!("invalid sheath state {state}").into());
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

// None below level 11; one minute per level above 10, up to the spell's ten minutes.
fn resurrection_sickness_micros(level: u32) -> Option<i64> {
    const MINUTE_MICROS: i64 = 60_000_000;
    (level >= 11).then(|| i64::from(level.min(20) - 10) * MINUTE_MICROS)
}

pub(crate) fn do_spirit_healer_res(ctx: &ReducerContext, guid: u64) -> Result<(), crate::Refusal> {
    use lyracore_shared::constants::player_flags;
    let entities = ctx.db.game_world_entity();
    let mut player = entities
        .guid()
        .find(guid)
        .ok_or_else(|| "caller not in world".to_string())?;
    if !player.dead || player.player_flags & player_flags::GHOST == 0 {
        return Err("caller is not a ghost".into());
    }
    let (health, power) = spirit_res_vitals(player.max_health, player.max_power);
    let player_level = player.level;
    player.health = health;
    player.power = power;
    player.update(ctx);
    crate::ghost::finish_resurrection(ctx, guid);
    crate::items::apply_spirit_healer_durability_loss(ctx, guid);
    let Some(sickness_micros) = resurrection_sickness_micros(player_level) else {
        return Ok(());
    };
    crate::spell::apply_spell_auras(ctx, RESURRECTION_SICKNESS_SPELL, guid, 1, 1);
    let expires_at = spacetimedb::Timestamp::from_micros_since_unix_epoch(
        crate::time::now(ctx).to_micros_since_unix_epoch() + sickness_micros,
    );
    let sickness: Vec<_> = ctx
        .db
        .game_aura()
        .by_target()
        .filter(&guid)
        .filter(|aura| aura.spell_id == RESURRECTION_SICKNESS_SPELL)
        .collect();
    for mut aura in sickness {
        aura.expires_at = expires_at;
        aura.update(ctx);
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
            crate::time::now(ctx).to_micros_since_unix_epoch(),
            c.session_start_micros,
            c.played_total_secs,
        );
        c.session_start_micros = 0;
        c.resting = entity.resting;
        if set_offline {
            c.online = false;
            c.last_logout_micros = crate::time::now(ctx).to_micros_since_unix_epoch() as u64;
            crate::rest::materialize_on_logout(
                &mut c,
                crate::time::now(ctx).to_micros_since_unix_epoch(),
            );
        }
        chars.guid().update(c);
    }
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

    finish_logout(
        ctx,
        entity.guid,
        persisted_pending_ghost(entity.dead, entity.player_flags),
    );
}

pub(crate) fn remove_loading_character(ctx: &ReducerContext, guid: u64) {
    crate::hooks::fire_on_logout(
        ctx,
        &crate::hooks::LogoutPayload {
            character_guid: guid,
        },
    );
    let Some(mut character) = crate::helpers::character_by_guid(ctx, guid) else {
        return;
    };
    character.online = false;
    character.last_logout_micros = crate::time::now(ctx).to_micros_since_unix_epoch() as u64;
    character.pending_godmode = false;
    character.pending_run_speed_mult_bp = RUN_SPEED_BP_1X;
    crate::rest::materialize_on_logout(
        &mut character,
        crate::time::now(ctx).to_micros_since_unix_epoch(),
    );
    let keep_corpse = character.pending_ghost;
    ctx.db.game_character().guid().update(character);
    finish_logout(ctx, guid, keep_corpse);
}

fn finish_logout(ctx: &ReducerContext, guid: u64, keep_corpse: bool) {
    crate::combat::disengage(ctx, guid);
    crate::trade::cancel_trade_for(ctx, guid);
    if !keep_corpse {
        crate::Corpse::delete(ctx, crate::corpse::corpse_guid_for(guid));
    }
    crate::spell::clear_resurrect_request_and_option(ctx, guid);
    crate::spell::break_stealth(ctx, guid);
    crate::creatures::despawn_pets(ctx, guid);
    for row in ctx
        .db
        .game_character_buyback()
        .by_player_guid()
        .filter(&guid)
        .collect::<Vec<_>>()
    {
        ctx.db.game_character_buyback().id().delete(row.id);
    }
    crate::motion::drop_pending(ctx, guid);
    ctx.db.game_entity_motion().guid().delete(guid);
    crate::WorldEntity::delete(ctx, guid);
}

pub(crate) fn apply_gossip_select(
    ctx: &ReducerContext,
    character_guid: u64,
    npc_guid: u64,
    selection: crate::gossip::Selection,
) -> Result<u32, String> {
    let target_menu = crate::gossip::select_gate(ctx, character_guid, npc_guid, &selection)?;
    crate::hooks::fire_on_gossip_select(
        ctx,
        &crate::hooks::GossipSelectPayload {
            character_guid,
            npc_guid,
            option_id: selection.option_id,
            option_row_id: selection.row_id,
        },
    );
    Ok(target_menu)
}

#[reducer(client_disconnected)]
pub fn on_disconnect(ctx: &ReducerContext) {
    crate::native::close(ctx);
    if let Some(entity) = entity_by_owner(ctx, ctx.sender()) {
        if !crate::native::is_player(ctx, entity.guid) {
            remove_live_character(ctx, entity);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        accrue_played_on_persist, can_inspect, ghost_restored_fields, is_cross_map_teleport,
        movement_violation, persisted_gm_playtest, persisted_pending_ghost, plan_movement,
        resolve_environmental_damage, resurrection_sickness_micros, snapshot_needs_persist,
        spirit_res_vitals, zone_transition, MovementDelta, INSPECT_RANGE_SQ, MOVE_VIOLATION_SPEED,
        MOVE_VIOLATION_TELEPORT, PERSIST_MAX_DRIFT_YD, RUN_SPEED_BP_1X,
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
        assert!(!snapshot_needs_persist(
            HEARTBEAT, false, false, false, 16.0
        ));
        assert!(snapshot_needs_persist(
            HEARTBEAT, false, false, false, 16.01
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
    fn plan_movement_persists_observable_changes_and_never_gates_the_relay() {
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
        for &(grid, flags, state, drift, persist) in &[
            (false, false, false, 0.0, false),
            (true, false, false, 0.0, true),
            (false, true, false, 0.0, true),
            (false, false, true, 0.0, true),
            (false, false, false, 17.0, true),
        ] {
            let plan = plan_movement(HEARTBEAT, grid, flags, state, drift, false, &stationary);
            assert_eq!(plan.persist_entity, persist);
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
    fn movement_clock_wrap_accepts_new_packets_and_refuses_stale_ones() {
        let accepted = |last, next| super::movement_is_accepted(last, next, 0.0, 0.0, 0.0, 0.0);
        assert!(accepted(0, u32::MAX - 10));
        assert!(accepted(u32::MAX - 10, 20));
        assert!(!accepted(20, u32::MAX - 10));
        assert!(!accepted(20, 20));
        assert!(!accepted(20, 19));
        assert!(accepted(20, 21));
    }

    #[test]
    fn movement_with_a_non_finite_coordinate_is_refused() {
        assert!(super::movement_is_accepted(10, 11, 1.0, 2.0, 3.0, 0.5));
        assert!(!super::movement_is_accepted(
            10,
            11,
            f32::NAN,
            2.0,
            3.0,
            0.5
        ));
        assert!(!super::movement_is_accepted(
            10,
            11,
            1.0,
            f32::INFINITY,
            3.0,
            0.5
        ));
        assert!(!super::movement_is_accepted(
            10,
            11,
            1.0,
            2.0,
            f32::NEG_INFINITY,
            0.5
        ));
        assert!(!super::movement_is_accepted(
            10,
            11,
            1.0,
            2.0,
            3.0,
            f32::NAN
        ));
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
    fn spirit_healer_res_restores_half_vitals() {
        assert_eq!(spirit_res_vitals(200, 80), (100, 40));
        assert_eq!(spirit_res_vitals(199, 81), (99, 40));
        assert_eq!(spirit_res_vitals(1, 0), (1, 0));
        assert_eq!(
            spirit_res_vitals(0, 0),
            (1, 0),
            "the .max(1) floor keeps a 0-max res alive"
        );
    }

    #[test]
    fn resurrection_sickness_lasts_a_minute_per_level_above_ten_up_to_ten_minutes() {
        const MINUTE: i64 = 60_000_000;
        assert_eq!(resurrection_sickness_micros(10), None);
        assert_eq!(resurrection_sickness_micros(11), Some(MINUTE));
        assert_eq!(resurrection_sickness_micros(15), Some(5 * MINUTE));
        assert_eq!(resurrection_sickness_micros(20), Some(10 * MINUTE));
        assert_eq!(resurrection_sickness_micros(60), Some(10 * MINUTE));
    }
}
