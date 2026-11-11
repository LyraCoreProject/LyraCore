use lyracore_protocol::codec::ClientMovement;
use lyracore_shared::constants::sheath_state;
use lyracore_shared::movement_flags;
use spacetimedb::{table, Identity, ReducerContext, Table};

use crate::character::{persist_entity, RUN_SPEED_BP_1X};
use crate::game_character;

mod character_entity;
mod placement;
mod simulation;

pub(crate) use character_entity::build_player_entity;
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
    // SpacetimeDB 2.14.0 serves this subscription only with three equality-indexed columns.
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
    #[default(0u32)]
    pub sheet_ap_modifiers: u32,
    #[default(0u32)]
    pub sheet_ranged_ap_modifiers: u32,
    #[default(0.0f32)]
    pub sheet_ranged_dmg_min_fraction: f32,
    #[default(0.0f32)]
    pub sheet_ranged_dmg_max_fraction: f32,
    #[default(0u32)]
    pub sheet_ranged_crit_bp: u32,
}

impl WorldEntity {
    /// A unit at `pose` with every other column at its neutral value; builders override the rest.
    pub(crate) fn blank(guid: u64, map_id: u32, instance_id: u64, pose: [f32; 4]) -> Self {
        let [x, y, z, orientation] = pose;
        let mut entity = Self {
            guid,
            owner_identity: Identity::ZERO,
            account_id: 0,
            map_id,
            x: 0.0,
            y: 0.0,
            z: 0.0,
            orientation,
            grid_x: 0,
            grid_y: 0,
            cell: 0,
            last_move_ms: 0,
            type_mask: 0,
            entry: 0,
            scale_x: 1.0,
            health: 0,
            max_health: 0,
            power: 0,
            max_power: 0,
            level: 0,
            faction_template: 0,
            unit_bytes_0: 0,
            display_id: 0,
            native_display_id: 0,
            unit_flags: 0,
            base_attack_time_ms: crate::combat::DEFAULT_ATTACK_TIME_MS,
            dynamic_flags: 0,
            dead: false,
            player_bytes: 0,
            player_bytes_2: 0,
            player_bytes_3: 0,
            player_flags: 0,
            xp: 0,
            next_level_xp: 0,
            target_guid: 0,
            money: 0,
            unit_bytes_1: 0,
            unit_bytes_2: 0,
            strength: 0,
            agility: 0,
            stamina: 0,
            intellect: 0,
            spirit: 0,
            npc_flags: 0,
            armor: 0,
            leg_ends_ms: 0,
            wp_target: 0,
            movement_flags: 0,
            combat_until_ms: 0,
            pickpocketed: false,
            next_swing_spell: 0,
            overpower_until_ms: 0,
            revenge_until_ms: 0,
            stance: 0,
            owner_guid: 0,
            skinned: false,
            mana_regen_paused_until_ms: 0,
            death_expire_micros: 0,
            instance_id,
            run_speed_mult_bp: RUN_SPEED_BP_1X,
            godmode: false,
            resting: false,
            sheet_str_bonus: 0,
            sheet_agi_bonus: 0,
            sheet_sta_bonus: 0,
            sheet_int_bonus: 0,
            sheet_spi_bonus: 0,
            sheet_ap_base: 0,
            sheet_ap_mods: 0,
            sheet_ap_modifiers: 0,
            sheet_ranged_ap_modifiers: 0,
            sheet_ranged_dmg_min_fraction: 0.0,
            sheet_ranged_dmg_max_fraction: 0.0,
            sheet_ranged_crit_bp: 0,
            sheet_dmg_min: 0,
            sheet_dmg_max: 0,
            sheet_crit_bp: 0,
            bank_bag_slots: 0,
            mount_display_id: 0,
            zone_id: 0,
            sheet_ranged_ap: 0,
            sheet_ranged_dmg_min: 0,
            sheet_ranged_dmg_max: 0,
            regeneration_pending: false,
            combat_active: false,
            simulation_indexed: false,
        };
        entity.place(x, y, z, MoveClock::Unchanged).unexplored();
        entity
    }

    pub(crate) fn is_player(&self) -> bool {
        self.type_mask & lyracore_shared::constants::type_mask::PLAYER_BIT != 0
    }

    pub(crate) fn race(&self) -> u8 {
        (self.unit_bytes_0 & 0xFF) as u8
    }

    pub(crate) fn class(&self) -> u8 {
        ((self.unit_bytes_0 >> 8) & 0xFF) as u8
    }

    /// The shapeshift form, byte 2 of `unit_bytes_1`.
    pub(crate) fn form(&self) -> u32 {
        self.unit_bytes_1 >> 16 & 0xff
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
        crate::modifiers::effective_move_speed(ctx, guid, lyracore_shared::constants::speeds::RUN);
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

pub(crate) fn apply_zone_transition(
    ctx: &ReducerContext,
    entity: &mut WorldEntity,
    resolved: Option<u32>,
) {
    let Some(zone) = resolved.filter(|zone| *zone != entity.zone_id) else {
        return;
    };
    entity.zone_id = zone;
    if let Some(mut character) = crate::helpers::character_by_guid(ctx, entity.guid) {
        character.zone_id = zone;
        ctx.db.game_character().guid().update(character);
    }
}

/// Moves a Character to `pose`; a `teleport_gate` refusal leaves everything unchanged.
pub(crate) fn teleport(
    ctx: &ReducerContext,
    player_guid: u64,
    map_id: u32,
    instance_id: u64,
    pose: [f32; 4],
) -> Result<(), crate::actor::ActionRefusal> {
    relocate(ctx, player_guid, map_id, instance_id, pose, None)
}

/// Carries a Passenger to `map_id` with its Transport; `pose` is its world pose there.
pub(crate) fn teleport_aboard(
    ctx: &ReducerContext,
    map_id: u32,
    pose: [f32; 4],
    aboard: crate::transport::Aboard,
) -> Result<(), crate::actor::ActionRefusal> {
    let guid = aboard.passenger.character_guid;
    relocate(ctx, guid, map_id, 0, pose, Some(aboard))
}

/// The Gate `teleport` applies; call it first only to refuse before writes of your own.
pub(crate) fn teleport_gate(
    ctx: &ReducerContext,
    player_guid: u64,
    map_id: u32,
    instance_id: u64,
) -> Result<Option<Identity>, crate::actor::ActionRefusal> {
    use crate::actor::{ActionRefusal, ActionRefusalKind};

    let destination =
        crate::native::directory::transfer_destination(ctx, player_guid, map_id, instance_id);
    let allowed = destination.map_or_else(
        || crate::native::directory::owns_map(ctx, map_id),
        |world| {
            crate::peer::is_peer(ctx, world) && crate::native::has_world_session(ctx, player_guid)
        },
    );
    if !allowed {
        return Err(ActionRefusal::new(
            ActionRefusalKind::OtherPartition,
            "destination requires a World Session Transfer or a locally owned map",
        ));
    }
    if crate::taxi::is_in_flight(ctx, player_guid) {
        return Err(ActionRefusal::new(
            ActionRefusalKind::CannotAct,
            "Character is in flight",
        ));
    }
    let entity = ctx
        .db
        .game_world_entity()
        .guid()
        .find(player_guid)
        .ok_or_else(|| {
            ActionRefusal::new(ActionRefusalKind::MissingActor, "Character is not in world")
        })?;
    if (entity.map_id != map_id || destination.is_some())
        && (crate::native::realm_petition::pending(ctx, player_guid)
            || crate::native::realm_auction::pending(ctx, player_guid))
    {
        return Err(ActionRefusal::new(
            ActionRefusalKind::CannotAct,
            "Character transaction is settling",
        ));
    }
    Ok(destination)
}

fn relocate(
    ctx: &ReducerContext,
    player_guid: u64,
    map_id: u32,
    instance_id: u64,
    [x, y, z, o]: [f32; 4],
    aboard: Option<crate::transport::Aboard>,
) -> Result<(), crate::actor::ActionRefusal> {
    let destination = teleport_gate(ctx, player_guid, map_id, instance_id)?;
    let entities = ctx.db.game_world_entity();
    let e = entities
        .guid()
        .find(player_guid)
        .expect("teleport Gate requires a live entity");
    let recipient_identity = e.owner_identity;
    let cross_map = e.map_id != map_id || destination.is_some();
    let sessionless = crate::helpers::character_by_guid(ctx, player_guid)
        .is_some_and(|character| !character.online);
    let changes_partition = e.map_id != map_id || e.instance_id != instance_id;
    if changes_partition {
        crate::duel::interrupt_duel_for(ctx, player_guid);
        crate::trade::cancel_trade_for(ctx, player_guid);
    }
    let teleport_counter = crate::native::teleport::begin(ctx, player_guid, cross_map);
    // Drop queued pre-teleport motion so the next publish cannot snap peers back.
    crate::motion::drop_pending(ctx, player_guid);
    crate::transport::leave(ctx, player_guid);

    let destination_zone = crate::terrain::zone_id_at(ctx, map_id, x, y);

    if cross_map {
        persist_entity(ctx, &e, false);
        crate::combat::disengage(ctx, player_guid);
        crate::spell::break_stealth(ctx, player_guid);
    }
    if cross_map && !sessionless {
        crate::WorldEntity::delete(ctx, player_guid);
    } else {
        if cross_map {
            ctx.db.game_entity_motion().guid().delete(player_guid);
        }
        let Some(mut e) = entities.guid().find(player_guid) else {
            return Ok(());
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

    crate::creatures::leg::halt_at(ctx, player_guid, map_id, instance_id, (x, y, z));

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
    crate::native::teleport::changed(ctx, &event, teleport_counter, aboard.as_ref());
    if let Some(aboard) = aboard.filter(|_| cross_map && !sessionless) {
        crate::transport::reboard(ctx, aboard.passenger);
    }

    if changes_partition {
        crate::instance::reconcile_instance_removal(ctx, player_guid);
    }
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

/// `relay_body` is the MovementInfo the motion Relay sends to observers unchanged; None when the
/// Native Host already relayed it.
pub(crate) fn apply_movement_update(
    ctx: &ReducerContext,
    mut mover: WorldEntity,
    opcode: u16,
    movement: &ClientMovement,
    relay_body: Option<Vec<u8>>,
) -> Result<(), String> {
    let info = movement.info();
    let (x, y, z) = (info.position.x, info.position.y, info.position.z);
    let (o, move_time_ms) = (info.orientation, info.timestamp);
    if crate::taxi::is_in_flight(ctx, mover.guid) {
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
        let dmg = fall_damage(movement.fall_time_ms(), mover.max_health);
        let (health, lethal) = resolve_environmental_damage(dmg, mover.health);
        mover.health = health;
        fall_lethal = lethal;
        crate::native::environment::fall_damage(ctx, mover.guid, dmg);
    }
    crossing.explore(ctx, &mut mover);
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
        let submerged = crate::terrain::is_submerged(mover.z, liquid_level, mover.movement_flags);
        crate::breath::update_breath_edge(ctx, &mover, submerged);
    }
    if crate::fatigue::feels_fatigue(&mover) && (move_time_ms / 1000 != old_move_ms / 1000) {
        let deep_water_level =
            crate::terrain::deep_water_level_at(ctx, mover.map_id, mover.x, mover.y);
        let in_deep_water =
            crate::terrain::is_in_deep_water(mover.z, deep_water_level, mover.movement_flags);
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
    let moved = delta.moved();

    if snapshot_needs_persist(
        opcode,
        grid_x != old_gx || grid_y != old_gy,
        mover.movement_flags != old_flags,
        mover.resting != old_resting || mover.health != old_health,
        drift_sq,
    ) {
        mover.update(ctx);
    }
    if fall_lethal {
        crate::combat::kill_player(ctx, mover_guid, mover_guid);
    }

    // Dismount after the entity write so its projection cannot be overwritten by the old snapshot.
    if mount_indoor_due && !crate::mount::mount_cast_allowed_here(ctx, map_id, x, y, z) {
        crate::mount::dismount(ctx, mover_guid);
    }

    if moved {
        crate::spell::break_channel(ctx, mover_guid);
    }
    // Use client flags: a late stop can change stored position without breaking Shadowmeld.
    if breaks_on_move {
        crate::spell::break_auras_on_move(ctx, mover_guid);
    }

    if moved && boarding == crate::transport::Boarding::Ashore {
        score_and_log_movement(ctx, mover_guid, &delta);
    }

    if let Some(relay_body) = relay_body {
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

/// The stand state is the low byte of `unit_bytes_1`.
pub(crate) const STAND_STATE_MASK: u32 = 0xFF;
pub(crate) const STAND_STATE_STAND: u32 = 0;
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
    let packed = (actor.unit_bytes_1 & !STAND_STATE_MASK) | u32::from(state);
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
    let state = target.unit_bytes_1 & STAND_STATE_MASK;
    if !target.is_player()
        || target.mount_display_id != 0
        || state == STAND_STATE_STAND
        || state == STAND_STATE_DEAD
    {
        return;
    }
    target.unit_bytes_1 &= !STAND_STATE_MASK;
    target.update(ctx);
}

pub(crate) fn sweep_expired(ctx: &ReducerContext) {
    crate::gc::reap_expired!(ctx, game_movement_violation, crate::EVENT_TTL_MICROS);
    let cutoff = crate::time::now(ctx).to_micros_since_unix_epoch() - crate::EVENT_TTL_MICROS;
    let teleports = ctx.db.game_teleport_event();
    let stale: Vec<u64> = teleports
        .iter()
        .filter(|event| (event.created_micros as i64) < cutoff)
        .map(|event| event.id)
        .collect();
    for id in stale {
        teleports.id().delete(id);
    }
}

// Client fall_time_ms includes jump ascent; gravity is about 19.29 yd/s^2.
fn fall_damage(fall_time_ms: u32, max_health: u32) -> u32 {
    let t = fall_time_ms as f32 / 1000.0;
    let height_yd = 0.5 * 19.29 * t * t;
    let perc = 0.018 * height_yd - 0.2426;
    if perc <= 0.0 {
        return 0;
    }
    (max_health as f32 * perc.min(1.0)) as u32
}

#[cfg(test)]
mod tests {
    use super::{
        fall_damage, movement_violation, resolve_environmental_damage, snapshot_needs_persist,
        MOVE_VIOLATION_SPEED, MOVE_VIOLATION_TELEPORT, PERSIST_MAX_DRIFT_YD,
    };

    const HEARTBEAT: u16 = lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT as u16;

    #[test]
    fn fall_damage_curve() {
        assert_eq!(fall_damage(0, 100), 0);
        assert_eq!(fall_damage(800, 100), 0);
        assert_eq!(fall_damage(1_180, 100), 0);
        assert_eq!(fall_damage(1_500, 100), 14);
        assert_eq!(fall_damage(1_500, 1_000), 148);
        assert_eq!(fall_damage(10_000, 100), 100);
    }

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
}
