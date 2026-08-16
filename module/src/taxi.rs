//! Taxi discovery and direct-flight activation. This module is the policy boundary for resolving a
//! flight master, reading or adding character progression, validating supported direct routes, and
//! atomically starting a paid flight. The gateway only requests an operation and encodes its reply
//! into the vanilla wire format. [entity]

use spacetimedb::{table, ReducerContext, ScheduleAt, Table, TimeDuration};

use crate::{
    game_character, game_taxi_node, game_taxi_path, game_taxi_path_node, game_world_entity,
    GameTaxiNode, GameTaxiPath, GameTaxiPathNode, WorldEntity,
};

/// Normal NPC interaction distance: (10 yd)², shared in value with vendor/trainer/quest gates.
const TAXI_INTERACTION_RANGE_SQ: f32 = 100.0;
/// A spawned flight master resolves to the nearest catalogue node on its map. Real DBC node
/// positions and creature spawns are close but not necessarily identical, so use the same 10-yard
/// tolerance as the player-to-NPC interaction instead of exact float equality.
const TAXI_NODE_RESOLVE_RANGE_SQ: f32 = 100.0;

/// One durable known-node record. `node_id` is the server-side storage id; the client-facing id is
/// resolved only when an operation reply is built. The logical key is `(character_guid, node_id)`;
/// reducers insert only after an indexed per-character dedup scan, and SpacetimeDB serializes the
/// transaction so concurrent opens cannot create two rows. [entity]
#[table(
    accessor = game_character_taxi_node,
    index(accessor = by_character, btree(columns = [character_guid]))
)]
pub struct CharacterTaxiNode {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub character_guid: u64,
    pub node_id: u32,
}

/// One authoritative in-progress direct flight. The character guid is the primary key, so the
/// serialized reducer transaction can never create two active flights or debit one activation
/// twice. Scheduled progression and landing extend/consume this row in the next slice; activation
/// records only the immutable route, mount, fare, and starting cursor. [entity]
#[derive(Clone)]
#[table(accessor = game_active_taxi_flight)]
pub struct ActiveTaxiFlight {
    #[primary_key]
    pub character_guid: u64,
    pub path_id: u32,
    pub source_node_id: u32,
    pub destination_node_id: u32,
    pub mount_display_id: u32,
    pub fare: u32,
    pub current_node_index: u32,
    pub started_micros: i64,
}

/// Scheduled authority loop for one passenger. The key is the character guid, so activation can
/// arm exactly one loop and landing can remove it without a scan.
#[table(accessor = game_taxi_flight_schedule, scheduled(advance_taxi_flight))]
pub struct TaxiFlightSchedule {
    #[primary_key]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}

/// Dedicated passenger spline carrier. Unlike ordinary player motion it includes the passenger,
/// and unlike `game_creature_spline` it never lends player state to the creature movement engine.
/// The row is refreshed whenever the authoritative cursor changes AOI cell, which lets a relog or
/// newly-entered observer receive the remaining route from the cell that actually owns it.
#[table(accessor = game_taxi_passenger_spline, public)]
pub struct TaxiPassengerSpline {
    #[primary_key]
    pub character_guid: u64,
    pub map_id: u32,
    pub instance_id: u64,
    pub grid_x: i32,
    pub grid_y: i32,
    pub cell: i64,
    pub start_x: f32,
    pub start_y: f32,
    pub start_z: f32,
    /// Flattened x/y/z triples, in travel order.
    pub points: Vec<f32>,
    pub duration_ms: u32,
    pub spline_id: u32,
}

crate::character_owned!(delete, fn sweep_delete_game_active_taxi_flight(ctx, character_guid) {
    ctx.db.game_active_taxi_flight().character_guid().delete(character_guid);
});
// Baseline direct routes never cross a supported shard boundary. A transfer must start without an
// active flight; carrying the row would strand a route whose NPC/catalogue partition was left behind.
crate::character_owned!(not_transported, fn sweep_transfer_game_active_taxi_flight());

crate::character_owned!(delete, fn sweep_delete_game_taxi_flight_schedule(ctx, character_guid) {
    ctx.db.game_taxi_flight_schedule().scheduled_id().delete(character_guid);
});
crate::character_owned!(not_transported, fn sweep_transfer_game_taxi_flight_schedule());

crate::character_owned!(delete, fn sweep_delete_game_taxi_passenger_spline(ctx, character_guid) {
    ctx.db.game_taxi_passenger_spline().character_guid().delete(character_guid);
});
crate::character_owned!(not_transported, fn sweep_transfer_game_taxi_passenger_spline());

crate::character_owned!(delete, fn sweep_delete_game_character_taxi_node(ctx, character_guid) {
    let known = ctx.db.game_character_taxi_node();
    for row in known.by_character().filter(&character_guid).collect::<Vec<_>>() {
        known.id().delete(row.id);
    }
});

// Taxi discoveries are durable progression. Surrogate ids are local to one database and must be
// minted again when the character crosses a shard boundary.
crate::character_owned!(transfer, fn sweep_transfer_game_character_taxi_node(ctx, character_guid, io) {
    table = game_character_taxi_node,
    by = by_character,
    remint = id,
});

/// The result mailbox for a cohesive taxi operation. `request_id` lets the gateway distinguish the
/// committed answer from an older cache row. Failed gameplay gates are represented by `accepted =
/// false` and COMMIT, rather than reducer errors that would roll the reply back. [server]
#[table(
    accessor = game_taxi_service_reply,
    index(accessor = by_character, btree(columns = [character_guid]))
)]
pub struct TaxiServiceReply {
    #[primary_key]
    pub request_id: u64,
    pub character_guid: u64,
    pub operation: u8,
    pub npc_guid: u64,
    pub accepted: bool,
    pub known: bool,
    pub source_client_node_id: u32,
    pub available_client_node_ids: Vec<u32>,
    pub refusal: String,
    pub created_micros: i64,
    /// Stable primitive result for activation replies. Status/open callers ignore it. End-appended
    /// with a default so publishing over the Ticket 02 mailbox schema is migration-safe.
    #[default(0)]
    pub result_code: u8,
}

crate::character_owned!(delete, fn sweep_delete_game_taxi_service_reply(ctx, character_guid) {
    let replies = ctx.db.game_taxi_service_reply();
    for reply in replies.by_character().filter(&character_guid).collect::<Vec<_>>() {
        replies.request_id().delete(reply.request_id);
    }
});
crate::character_owned!(not_transported, fn sweep_transfer_game_taxi_service_reply());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaxiGateDenied {
    PlayerMissing,
    PlayerDead,
    FlightMasterMissing,
    FlightMasterDead,
    NotFlightMaster,
    DifferentPartition,
    OutOfRange,
    Hostile,
    NotInteractable,
    NoSourceNode,
    UnsupportedInstance,
    PlayerBusy,
    PlayerAlreadyMounted,
    PlayerMoving,
    PlayerShapeShifted,
    PlayerNotStanding,
    SourceMismatch,
    DestinationMissing,
    DestinationUnknown,
    SameNode,
    NoDirectPath,
    MissingMount,
    NotEnoughMoney,
}

impl TaxiGateDenied {
    fn message(self) -> &'static str {
        match self {
            Self::PlayerMissing => "player not in world",
            Self::PlayerDead => "dead players cannot use a taxi",
            Self::FlightMasterMissing => "no such flight master",
            Self::FlightMasterDead => "flight master is dead",
            Self::NotFlightMaster => "target is not a flight master",
            Self::DifferentPartition => "flight master is in another world partition",
            Self::OutOfRange => "flight master is out of range",
            Self::Hostile => "flight master refuses interaction",
            Self::NotInteractable => "flight master is not interactable",
            Self::NoSourceNode => "flight master has no nearby taxi node",
            Self::UnsupportedInstance => "taxi routes are unavailable in instances",
            Self::PlayerBusy => "player is busy",
            Self::PlayerAlreadyMounted => "player is already mounted",
            Self::PlayerMoving => "player must stop moving",
            Self::PlayerShapeShifted => "player is shape-shifted",
            Self::PlayerNotStanding => "player is not standing",
            Self::SourceMismatch => "packet source does not match this flight master",
            Self::DestinationMissing => "destination node does not exist",
            Self::DestinationUnknown => "destination node has not been visited",
            Self::SameNode => "source and destination are the same",
            Self::NoDirectPath => "no complete supported direct route",
            Self::MissingMount => "taxi node has no usable faction mount",
            Self::NotEnoughMoney => "not enough money for taxi fare",
        }
    }

    fn activate_result_code(self) -> u8 {
        use lyracore_shared::constants::taxi_protocol as wire;
        match self {
            Self::PlayerMissing => wire::ACTIVATE_UNSPECIFIED_SERVER_ERROR,
            Self::PlayerDead | Self::PlayerBusy | Self::UnsupportedInstance => {
                wire::ACTIVATE_PLAYER_BUSY
            }
            Self::PlayerAlreadyMounted => wire::ACTIVATE_PLAYER_ALREADY_MOUNTED,
            Self::PlayerMoving => wire::ACTIVATE_PLAYER_MOVING,
            Self::PlayerShapeShifted => wire::ACTIVATE_PLAYER_SHAPE_SHIFTED,
            Self::PlayerNotStanding => wire::ACTIVATE_NOT_STANDING,
            Self::FlightMasterMissing
            | Self::FlightMasterDead
            | Self::NotFlightMaster
            | Self::NotInteractable
            | Self::Hostile => wire::ACTIVATE_NO_VENDOR_NEARBY,
            Self::DifferentPartition | Self::OutOfRange => wire::ACTIVATE_TOO_FAR_AWAY,
            Self::SourceMismatch | Self::DestinationUnknown => wire::ACTIVATE_NOT_VISITED,
            Self::SameNode => wire::ACTIVATE_SAME_NODE,
            Self::NoSourceNode
            | Self::DestinationMissing
            | Self::NoDirectPath
            | Self::MissingMount => wire::ACTIVATE_NO_SUCH_PATH,
            Self::NotEnoughMoney => wire::ACTIVATE_NOT_ENOUGH_MONEY,
        }
    }
}

fn squared_distance(a: (f32, f32, f32), b: (f32, f32, f32)) -> f32 {
    let (dx, dy, dz) = (b.0 - a.0, b.1 - a.1, b.2 - a.2);
    dx * dx + dy * dy + dz * dz
}

fn flight_master_is_selectable(unit_flags: u32) -> bool {
    unit_flags & lyracore_shared::constants::unit_flags::NOT_SELECTABLE == 0
}

/// Resolve every fact needed by status/open at one chokepoint. Source lookup deliberately returns
/// the storage-id row; only operation results expose `client_node_id`.
fn resolve_flight_master(
    ctx: &ReducerContext,
    character_guid: u64,
    npc_guid: u64,
) -> Result<(WorldEntity, GameTaxiNode), TaxiGateDenied> {
    let entities = ctx.db.game_world_entity();
    let player = crate::helpers::acting_entity_by_guid(ctx, character_guid)
        .ok_or(TaxiGateDenied::PlayerMissing)?;
    if player.dead || player.health == 0 {
        return Err(TaxiGateDenied::PlayerDead);
    }
    if player.instance_id != 0 {
        return Err(TaxiGateDenied::UnsupportedInstance);
    }
    let npc = entities
        .guid()
        .find(npc_guid)
        .ok_or(TaxiGateDenied::FlightMasterMissing)?;
    if npc.dead || npc.health == 0 {
        return Err(TaxiGateDenied::FlightMasterDead);
    }
    if npc.is_player() || npc.npc_flags & lyracore_shared::constants::npc_flags::TAXI == 0 {
        return Err(TaxiGateDenied::NotFlightMaster);
    }
    if !flight_master_is_selectable(npc.unit_flags) {
        return Err(TaxiGateDenied::NotInteractable);
    }
    if npc.map_id != player.map_id || npc.instance_id != player.instance_id {
        return Err(TaxiGateDenied::DifferentPartition);
    }
    if crate::helpers::dist_sq(&player, &npc) > TAXI_INTERACTION_RANGE_SQ {
        return Err(TaxiGateDenied::OutOfRange);
    }
    if crate::reputation::npc_refuses_interaction(ctx, &npc, &player) {
        return Err(TaxiGateDenied::Hostile);
    }

    let source = ctx
        .db
        .game_taxi_node()
        .iter()
        .filter(|node| node.map_id == npc.map_id)
        .filter_map(|node| {
            let distance = squared_distance((npc.x, npc.y, npc.z), (node.x, node.y, node.z));
            (distance <= TAXI_NODE_RESOLVE_RANGE_SQ).then_some((distance, node))
        })
        .min_by(|(da, a), (db, b)| da.total_cmp(db).then_with(|| a.id.cmp(&b.id)))
        .map(|(_, node)| node)
        .ok_or(TaxiGateDenied::NoSourceNode)?;
    Ok((player, source))
}

/// The small state machine at the atomic activation boundary. It owns every player-local gate and
/// computes the purse/presentation mutation without touching storage. The reducer persists this
/// state only after all NPC, catalogue, topology, and discovery checks have also succeeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ActivationPlayerState {
    has_active_flight: bool,
    money: u32,
    mount_display_id: u32,
    movement_flags: u32,
    stance: u8,
    stand_state: u8,
    unit_flags: u32,
}

impl ActivationPlayerState {
    fn from_world(ctx: &ReducerContext, player: &WorldEntity) -> Self {
        Self {
            has_active_flight: ctx
                .db
                .game_active_taxi_flight()
                .character_guid()
                .find(player.guid)
                .is_some(),
            money: player.money,
            mount_display_id: player.mount_display_id,
            movement_flags: player.movement_flags,
            stance: player.stance,
            stand_state: (player.unit_bytes_1 & 0xFF) as u8,
            unit_flags: player.unit_flags,
        }
    }

    fn validate_idle(self) -> Result<(), TaxiGateDenied> {
        if self.has_active_flight {
            return Err(TaxiGateDenied::PlayerBusy);
        }
        if self.mount_display_id != 0 {
            return Err(TaxiGateDenied::PlayerAlreadyMounted);
        }
        if self.movement_flags != 0 {
            return Err(TaxiGateDenied::PlayerMoving);
        }
        // Warrior stances remain taxi eligible; only the Druid-form range is a shape shift.
        if self.stance >= crate::spell::STANCE_BEAR {
            return Err(TaxiGateDenied::PlayerShapeShifted);
        }
        if self.stand_state != 0 {
            return Err(TaxiGateDenied::PlayerNotStanding);
        }
        if self.unit_flags & lyracore_shared::constants::unit_flags::IN_COMBAT != 0 {
            return Err(TaxiGateDenied::PlayerBusy);
        }
        Ok(())
    }

    fn activate(&mut self, fare: u32, mount_display_id: u32) -> Result<(), TaxiGateDenied> {
        self.validate_idle()?;
        if mount_display_id == 0 {
            return Err(TaxiGateDenied::MissingMount);
        }
        let remaining_money = debit_fare(self.money, fare)?;

        self.has_active_flight = true;
        self.money = remaining_money;
        self.mount_display_id = mount_display_id;
        self.unit_flags |= lyracore_shared::constants::unit_flags::TAXI_FLIGHT;
        Ok(())
    }
}

fn faction_mount_display(race: u8, source: &GameTaxiNode) -> u32 {
    if lyracore_shared::faction::team_for_race(race) == lyracore_shared::faction::TEAM_HORDE {
        source.mount_display_horde
    } else {
        source.mount_display_alliance
    }
}

fn debit_fare(money: u32, fare: u32) -> Result<u32, TaxiGateDenied> {
    money
        .checked_sub(fare)
        .ok_or(TaxiGateDenied::NotEnoughMoney)
}

fn direct_supported_path(
    ctx: &ReducerContext,
    source: &GameTaxiNode,
    destination: &GameTaxiNode,
) -> Result<GameTaxiPath, TaxiGateDenied> {
    let mut candidates = ctx
        .db
        .game_taxi_path()
        .by_source()
        .filter(&source.id)
        .filter(|path| path.destination_node_id == destination.id);
    let path = candidates.next().ok_or(TaxiGateDenied::NoDirectPath)?;
    if candidates.next().is_some() {
        return Err(TaxiGateDenied::NoDirectPath);
    }
    let mut points: Vec<_> = ctx
        .db
        .game_taxi_path_node()
        .by_path_id()
        .filter(&path.id)
        .collect();
    points.sort_by_key(|point| point.node_index);
    if !route_is_supported(source, destination, &points) {
        return Err(TaxiGateDenied::NoDirectPath);
    }
    Ok(path)
}

fn is_known(ctx: &ReducerContext, character_guid: u64, node_id: u32) -> bool {
    ctx.db
        .game_character_taxi_node()
        .by_character()
        .filter(&character_guid)
        .any(|row| row.node_id == node_id)
}

fn discover(ctx: &ReducerContext, character_guid: u64, node_id: u32) {
    if should_insert_discovery(is_known(ctx, character_guid, node_id)) {
        ctx.db.game_character_taxi_node().insert(CharacterTaxiNode {
            id: 0,
            character_guid,
            node_id,
        });
    }
}

fn should_insert_discovery(already_known: bool) -> bool {
    !already_known
}

/// A direct route is offerable only when all geometry exists in one hosted map and its point
/// indices are a complete zero-based sequence. This rules out missing point batches and every
/// loading-screen/cross-map approximation. Delays and point flags have client semantics this
/// baseline does not emulate, so routes containing either are rejected rather than approximated.
fn route_is_supported(
    source: &GameTaxiNode,
    destination: &GameTaxiNode,
    points: &[GameTaxiPathNode],
) -> bool {
    if source.map_id != destination.map_id || points.len() < 2 {
        return false;
    }
    points.iter().enumerate().all(|(index, point)| {
        point.map_id == source.map_id
            && point.node_index == index as u32
            && point.delay_ms == 0
            && point.flags == 0
    })
}

const TAXI_SPEED_YARDS_PER_SECOND: f64 = 32.0;
const TAXI_TICK_MICROS: i64 = 250_000;
const TAXI_MOVEMENT_FORWARD: u32 = 0x0000_0001;

#[derive(Clone, Copy, Debug, PartialEq)]
struct RoutePosition {
    point_index: usize,
    x: f32,
    y: f32,
    z: f32,
    orientation: f32,
    complete: bool,
}

fn segment_micros(a: &GameTaxiPathNode, b: &GameTaxiPathNode) -> i64 {
    let dx = f64::from(b.x - a.x);
    let dy = f64::from(b.y - a.y);
    let dz = f64::from(b.z - a.z);
    let travel = ((dx * dx + dy * dy + dz * dz).sqrt() / TAXI_SPEED_YARDS_PER_SECOND * 1_000_000.0)
        .round() as i64;
    travel.max(1)
}

fn route_duration_micros(points: &[GameTaxiPathNode]) -> Option<i64> {
    (points.len() >= 2).then(|| {
        points
            .windows(2)
            .map(|pair| segment_micros(&pair[0], &pair[1]))
            .fold(0i64, i64::saturating_add)
    })
}

/// Evaluate the route directly from authoritative elapsed time. No client delta or previous tick
/// participates, so delayed scheduler firings catch up instead of accumulating drift.
fn route_position(points: &[GameTaxiPathNode], elapsed_micros: i64) -> Option<RoutePosition> {
    let total = route_duration_micros(points)?;
    let elapsed = elapsed_micros.max(0);
    if elapsed >= total {
        let last = points.last()?;
        let prev = &points[points.len() - 2];
        return Some(RoutePosition {
            point_index: points.len() - 1,
            x: last.x,
            y: last.y,
            z: last.z,
            orientation: (last.y - prev.y).atan2(last.x - prev.x),
            complete: true,
        });
    }

    let mut segment_start = 0i64;
    for (index, pair) in points.windows(2).enumerate() {
        let duration = segment_micros(&pair[0], &pair[1]);
        if elapsed < segment_start.saturating_add(duration) {
            let travel_elapsed = elapsed.saturating_sub(segment_start);
            let t = (travel_elapsed as f64 / duration as f64).clamp(0.0, 1.0) as f32;
            return Some(RoutePosition {
                point_index: index,
                x: pair[0].x + (pair[1].x - pair[0].x) * t,
                y: pair[0].y + (pair[1].y - pair[0].y) * t,
                z: pair[0].z + (pair[1].z - pair[0].z) * t,
                orientation: (pair[1].y - pair[0].y).atan2(pair[1].x - pair[0].x),
                complete: false,
            });
        }
        segment_start = segment_start.saturating_add(duration);
    }
    None
}

fn ordered_route(ctx: &ReducerContext, path_id: u32) -> Option<Vec<GameTaxiPathNode>> {
    let mut points: Vec<_> = ctx
        .db
        .game_taxi_path_node()
        .by_path_id()
        .filter(&path_id)
        .collect();
    points.sort_by_key(|point| point.node_index);
    let complete = points.iter().enumerate().all(|(index, point)| {
        point.node_index == index as u32 && point.delay_ms == 0 && point.flags == 0
    });
    (points.len() >= 2 && complete).then_some(points)
}

fn remaining_spline(
    flight: &ActiveTaxiFlight,
    player: &WorldEntity,
    points: &[GameTaxiPathNode],
    position: RoutePosition,
    elapsed_micros: i64,
) -> TaxiPassengerSpline {
    let mut remaining = Vec::with_capacity((points.len() - position.point_index) * 3);
    for point in points.iter().skip(position.point_index + 1) {
        remaining.extend_from_slice(&[point.x, point.y, point.z]);
    }
    let duration_micros = route_duration_micros(points)
        .unwrap_or_default()
        .saturating_sub(elapsed_micros.max(0));
    TaxiPassengerSpline {
        character_guid: flight.character_guid,
        map_id: player.map_id,
        instance_id: player.instance_id,
        grid_x: player.grid_x,
        grid_y: player.grid_y,
        cell: lyracore_shared::spatial::grid_cell_id(player.grid_x, player.grid_y),
        start_x: position.x,
        start_y: position.y,
        start_z: position.z,
        points: remaining,
        duration_ms: (duration_micros / 1_000).clamp(1, i64::from(u32::MAX)) as u32,
        spline_id: (flight.started_micros as u64 as u32).wrapping_add(position.point_index as u32),
    }
}

fn write_passenger_spline(ctx: &ReducerContext, row: TaxiPassengerSpline) {
    let splines = ctx.db.game_taxi_passenger_spline();
    if splines.character_guid().find(row.character_guid).is_some() {
        splines.character_guid().update(row);
    } else {
        splines.insert(row);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FlightPresentation {
    mount_display_id: u32,
    unit_flags: u32,
    movement_flags: u32,
}

fn cleared_presentation(mut state: FlightPresentation) -> FlightPresentation {
    state.mount_display_id = 0;
    state.unit_flags &= !lyracore_shared::constants::unit_flags::TAXI_FLIGHT;
    state.movement_flags = 0;
    state
}

fn should_refresh_spline(old_grid: (i32, i32), new_grid: (i32, i32), crossed_point: bool) -> bool {
    crossed_point || old_grid != new_grid
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct LandingPosition {
    map_id: u32,
    x: f32,
    y: f32,
    z: f32,
    orientation: f32,
    grid_x: i32,
    grid_y: i32,
    cell: i64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct FlightEndPlan {
    landing: Option<LandingPosition>,
    instance_id: u64,
    zone_id: Option<u32>,
}

trait FlightEndSink {
    fn project_world(&mut self, landing: Option<LandingPosition>);
    fn project_character(
        &mut self,
        landing: LandingPosition,
        instance_id: u64,
        zone_id: Option<u32>,
    );
    fn drop_pending_motion(&mut self);
    fn clear_spline(&mut self);
    fn clear_schedule(&mut self);
    fn take_active(&mut self) -> bool;
}

fn apply_flight_end(sink: &mut impl FlightEndSink, plan: FlightEndPlan) {
    if !sink.take_active() {
        // Repair harmless orphan presentation rows left by an interrupted older build.
        sink.clear_spline();
        sink.clear_schedule();
        return;
    }
    sink.project_world(plan.landing);
    if let Some(landing) = plan.landing {
        sink.drop_pending_motion();
        sink.project_character(landing, plan.instance_id, plan.zone_id);
    }
    sink.clear_spline();
    sink.clear_schedule();
}

struct CtxFlightEndSink<'a> {
    ctx: &'a ReducerContext,
    player: Option<WorldEntity>,
    character_guid: u64,
}

impl FlightEndSink for CtxFlightEndSink<'_> {
    fn project_world(&mut self, landing: Option<LandingPosition>) {
        let Some(mut player) = self.player.take() else {
            return;
        };
        if let Some(landing) = landing {
            player.map_id = landing.map_id;
            player.x = landing.x;
            player.y = landing.y;
            player.z = landing.z;
            player.orientation = landing.orientation;
            player.grid_x = landing.grid_x;
            player.grid_y = landing.grid_y;
            player.cell = landing.cell;
        }
        let cleared = cleared_presentation(FlightPresentation {
            mount_display_id: player.mount_display_id,
            unit_flags: player.unit_flags,
            movement_flags: player.movement_flags,
        });
        player.mount_display_id = cleared.mount_display_id;
        player.unit_flags = cleared.unit_flags;
        player.movement_flags = cleared.movement_flags;
        self.ctx.db.game_world_entity().guid().update(player);
    }

    fn project_character(
        &mut self,
        landing: LandingPosition,
        instance_id: u64,
        zone_id: Option<u32>,
    ) {
        if let Some(mut character) =
            crate::helpers::character_by_guid(self.ctx, self.character_guid)
        {
            character.map_id = landing.map_id;
            character.x = landing.x;
            character.y = landing.y;
            character.z = landing.z;
            character.orientation = landing.orientation;
            character.pending_instance_id = instance_id;
            if let Some(zone_id) = zone_id {
                character.zone_id = zone_id;
            }
            self.ctx.db.game_character().guid().update(character);
        }
    }

    fn drop_pending_motion(&mut self) {
        crate::motion::drop_pending(self.ctx, self.character_guid);
    }

    fn clear_spline(&mut self) {
        self.ctx
            .db
            .game_taxi_passenger_spline()
            .character_guid()
            .delete(self.character_guid);
    }

    fn clear_schedule(&mut self) {
        self.ctx
            .db
            .game_taxi_flight_schedule()
            .scheduled_id()
            .delete(self.character_guid);
    }

    fn take_active(&mut self) -> bool {
        let rows = self.ctx.db.game_active_taxi_flight();
        let present = rows.character_guid().find(self.character_guid).is_some();
        if present {
            rows.character_guid().delete(self.character_guid);
        }
        present
    }
}

fn finish_flight(ctx: &ReducerContext, player: WorldEntity, plan: FlightEndPlan) {
    let character_guid = player.guid;
    apply_flight_end(
        &mut CtxFlightEndSink {
            ctx,
            player: Some(player),
            character_guid,
        },
        plan,
    );
}

fn clear_flight(ctx: &ReducerContext, player: WorldEntity) {
    finish_flight(
        ctx,
        player,
        FlightEndPlan {
            landing: None,
            instance_id: 0,
            zone_id: None,
        },
    );
}

fn exact_landing(destination: &GameTaxiNode, orientation: f32) -> LandingPosition {
    let (grid_x, grid_y) = lyracore_shared::spatial::grid_cell(destination.x, destination.y);
    LandingPosition {
        map_id: destination.map_id,
        x: destination.x,
        y: destination.y,
        z: destination.z,
        orientation,
        grid_x,
        grid_y,
        cell: lyracore_shared::spatial::grid_cell_id(grid_x, grid_y),
    }
}

fn land_flight(
    ctx: &ReducerContext,
    player: WorldEntity,
    destination: &GameTaxiNode,
    orientation: f32,
) {
    let landing = exact_landing(destination, orientation);
    let zone_id = crate::terrain::zone_id_at(ctx, destination.map_id, destination.x, destination.y);
    let instance_id = player.instance_id;
    finish_flight(
        ctx,
        player,
        FlightEndPlan {
            landing: Some(landing),
            instance_id,
            zone_id,
        },
    );
}

/// True for every inbound action while this character has module-owned taxi state.
pub(crate) fn is_in_flight(ctx: &ReducerContext, character_guid: u64) -> bool {
    ctx.db
        .game_active_taxi_flight()
        .character_guid()
        .find(character_guid)
        .is_some()
}

pub(crate) fn reject_action_while_in_flight(
    ctx: &ReducerContext,
    character_guid: u64,
) -> Result<(), String> {
    action_gate(is_in_flight(ctx, character_guid))
}

fn action_gate(active_flight: bool) -> Result<(), String> {
    if active_flight {
        Err("PLAYER_IN_TAXI_FLIGHT".to_string())
    } else {
        Ok(())
    }
}

pub(crate) fn movement_is_suppressed(ctx: &ReducerContext, character_guid: u64) -> bool {
    is_in_flight(ctx, character_guid)
}

fn available_client_nodes(
    ctx: &ReducerContext,
    character_guid: u64,
    source: &GameTaxiNode,
) -> Vec<u32> {
    let mut available = vec![source.client_node_id];
    for path in ctx.db.game_taxi_path().by_source().filter(&source.id) {
        let Some(destination) = ctx.db.game_taxi_node().id().find(path.destination_node_id) else {
            continue;
        };
        if !is_known(ctx, character_guid, destination.id) {
            continue;
        }
        let mut points: Vec<GameTaxiPathNode> = ctx
            .db
            .game_taxi_path_node()
            .by_path_id()
            .filter(&path.id)
            .collect();
        points.sort_by_key(|point| point.node_index);
        if route_is_supported(source, &destination, &points) {
            available.push(destination.client_node_id);
        }
    }
    available.sort_unstable();
    available.dedup();
    available
}

/// The gateway waits at most one second. A much longer fallback window preserves every plausible
/// in-flight reply while eventually collecting rows left behind by a crashed gateway.
const TAXI_REPLY_STALE_AFTER_MICROS: i64 = 60_000_000;

fn stale_reply_ids(replies: impl IntoIterator<Item = (u64, i64)>, now_micros: i64) -> Vec<u64> {
    let cutoff = now_micros.saturating_sub(TAXI_REPLY_STALE_AFTER_MICROS);
    replies
        .into_iter()
        .filter_map(|(request_id, created_micros)| (created_micros <= cutoff).then_some(request_id))
        .collect()
}

fn reply_belongs_to_character(reply: &TaxiServiceReply, character_guid: u64) -> bool {
    reply.character_guid == character_guid
}

fn write_reply(ctx: &ReducerContext, reply: TaxiServiceReply) {
    let replies = ctx.db.game_taxi_service_reply();
    let character_guid = reply.character_guid;
    let stale: Vec<_> = replies
        .by_character()
        .filter(&character_guid)
        .map(|row| (row.request_id, row.created_micros))
        .collect();
    for request_id in stale_reply_ids(stale, ctx.timestamp.to_micros_since_unix_epoch()) {
        replies.request_id().delete(request_id);
    }
    replies.insert(reply);
}

fn refused_reply(
    character_guid: u64,
    request_id: u64,
    operation: u8,
    npc_guid: u64,
    denied: TaxiGateDenied,
    created_micros: i64,
) -> TaxiServiceReply {
    TaxiServiceReply {
        character_guid,
        request_id,
        operation,
        npc_guid,
        accepted: false,
        known: false,
        source_client_node_id: 0,
        available_client_node_ids: Vec::new(),
        refusal: denied.message().to_string(),
        created_micros,
        result_code: denied.activate_result_code(),
    }
}

fn activation_reply(
    character_guid: u64,
    request_id: u64,
    npc_guid: u64,
    source_client_node_id: u32,
    created_micros: i64,
    attempt: Result<(), TaxiGateDenied>,
) -> TaxiServiceReply {
    let (accepted, result_code, refusal) = match attempt {
        Ok(()) => (
            true,
            lyracore_shared::constants::taxi_protocol::ACTIVATE_OK,
            "",
        ),
        Err(denied) => (false, denied.activate_result_code(), denied.message()),
    };
    TaxiServiceReply {
        request_id,
        character_guid,
        operation: lyracore_shared::constants::taxi_protocol::REPLY_ACTIVATE,
        npc_guid,
        accepted,
        known: false,
        source_client_node_id,
        available_client_node_ids: Vec::new(),
        refusal: refusal.to_string(),
        created_micros,
        result_code,
    }
}

/// Read-only status operation. A valid reply reports the persisted known bit and never discovers.
#[spacetimedb::reducer]
pub fn gw_taxi_node_status(
    ctx: &ReducerContext,
    character_guid: u64,
    npc_guid: u64,
    request_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let reply = match resolve_flight_master(ctx, character_guid, npc_guid) {
        Ok((_player, source)) => TaxiServiceReply {
            character_guid,
            request_id,
            operation: lyracore_shared::constants::taxi_protocol::REPLY_STATUS,
            npc_guid,
            accepted: true,
            known: is_known(ctx, character_guid, source.id),
            source_client_node_id: source.client_node_id,
            available_client_node_ids: Vec::new(),
            refusal: String::new(),
            created_micros: ctx.timestamp.to_micros_since_unix_epoch(),
            result_code: lyracore_shared::constants::taxi_protocol::ACTIVATE_OK,
        },
        Err(denied) => refused_reply(
            character_guid,
            request_id,
            lyracore_shared::constants::taxi_protocol::REPLY_STATUS,
            npc_guid,
            denied,
            ctx.timestamp.to_micros_since_unix_epoch(),
        ),
    };
    write_reply(ctx, reply);
    Ok(())
}

/// Open the direct-route map. Discovery and availability are computed in one transaction, so the
/// newly known source is present in the result immediately and repeated opens stay idempotent.
#[spacetimedb::reducer]
pub fn gw_open_taxi(
    ctx: &ReducerContext,
    character_guid: u64,
    npc_guid: u64,
    request_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let reply = match resolve_flight_master(ctx, character_guid, npc_guid) {
        Ok((_player, source)) => {
            discover(ctx, character_guid, source.id);
            TaxiServiceReply {
                character_guid,
                request_id,
                operation: lyracore_shared::constants::taxi_protocol::REPLY_OPEN,
                npc_guid,
                accepted: true,
                known: true,
                source_client_node_id: source.client_node_id,
                available_client_node_ids: available_client_nodes(ctx, character_guid, &source),
                refusal: String::new(),
                created_micros: ctx.timestamp.to_micros_since_unix_epoch(),
                result_code: lyracore_shared::constants::taxi_protocol::ACTIVATE_OK,
            }
        }
        Err(denied) => refused_reply(
            character_guid,
            request_id,
            lyracore_shared::constants::taxi_protocol::REPLY_OPEN,
            npc_guid,
            denied,
            ctx.timestamp.to_micros_since_unix_epoch(),
        ),
    };
    write_reply(ctx, reply);
    Ok(())
}

/// Activate one direct imported route. Every gameplay refusal commits a stable result mailbox row;
/// only operator/transport failures return `Err`. SpacetimeDB executes the validation, unique-row
/// insert, purse debit, presentation mutation, and reply write as one serialized transaction.
#[spacetimedb::reducer]
pub fn gw_activate_taxi(
    ctx: &ReducerContext,
    character_guid: u64,
    npc_guid: u64,
    source_client_node_id: u32,
    destination_client_node_id: u32,
    request_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let created_micros = ctx.timestamp.to_micros_since_unix_epoch();
    let attempt = (|| -> Result<(), TaxiGateDenied> {
        let (mut player, source) = resolve_flight_master(ctx, character_guid, npc_guid)?;
        let mut player_state = ActivationPlayerState::from_world(ctx, &player);
        player_state.validate_idle()?;
        if source.client_node_id != source_client_node_id {
            return Err(TaxiGateDenied::SourceMismatch);
        }
        let destination = ctx
            .db
            .game_taxi_node()
            .client_node_id()
            .find(destination_client_node_id)
            .ok_or(TaxiGateDenied::DestinationMissing)?;
        if source.id == destination.id {
            return Err(TaxiGateDenied::SameNode);
        }
        if !is_known(ctx, character_guid, destination.id) {
            return Err(TaxiGateDenied::DestinationUnknown);
        }
        let path = direct_supported_path(ctx, &source, &destination)?;
        ordered_route(ctx, path.id).ok_or(TaxiGateDenied::NoDirectPath)?;
        let mount_display_id = faction_mount_display(player.race(), &source);
        player_state.activate(path.fare, mount_display_id)?;

        let flight = ActiveTaxiFlight {
            character_guid,
            path_id: path.id,
            source_node_id: source.id,
            destination_node_id: destination.id,
            mount_display_id,
            fare: path.fare,
            current_node_index: 0,
            // Zero is the durable "paid but not presented" phase. The gateway queues the OK reply
            // and only then calls `gw_arm_taxi_flight`, preventing motion from overtaking it.
            started_micros: 0,
        };
        ctx.db.game_active_taxi_flight().insert(flight);
        player.money = player_state.money;
        ctx.db.game_world_entity().guid().update(player);
        Ok(())
    })();

    write_reply(
        ctx,
        activation_reply(
            character_guid,
            request_id,
            npc_guid,
            source_client_node_id,
            created_micros,
            attempt,
        ),
    );
    Ok(())
}

/// Enter the presentation/running phase after the gateway has queued `ACTIVATETAXIREPLY(OK)`.
/// Idempotency supplies crash recovery: a paid-but-unarmed row is resumed on the next login.
pub(crate) fn arm_taxi_flight(ctx: &ReducerContext, character_guid: u64) {
    let Some(mut flight) = ctx
        .db
        .game_active_taxi_flight()
        .character_guid()
        .find(character_guid)
    else {
        return;
    };
    if flight.started_micros != 0 {
        return;
    }
    let Some(mut player) = ctx.db.game_world_entity().guid().find(character_guid) else {
        return;
    };
    let Some(points) = ordered_route(ctx, flight.path_id) else {
        clear_flight(ctx, player);
        return;
    };
    let Some(destination) = ctx
        .db
        .game_taxi_node()
        .id()
        .find(flight.destination_node_id)
    else {
        clear_flight(ctx, player);
        return;
    };
    if destination.map_id != player.map_id
        || points.iter().any(|point| point.map_id != player.map_id)
    {
        clear_flight(ctx, player);
        return;
    }
    flight.started_micros = ctx.timestamp.to_micros_since_unix_epoch();
    player.mount_display_id = flight.mount_display_id;
    player.unit_flags |= lyracore_shared::constants::unit_flags::TAXI_FLIGHT;
    player.movement_flags = TAXI_MOVEMENT_FORWARD;
    let start = route_position(&points, 0).expect("validated route has a start");
    let spline = remaining_spline(&flight, &player, &points, start, 0);
    ctx.db
        .game_active_taxi_flight()
        .character_guid()
        .update(flight);
    ctx.db.game_world_entity().guid().update(player);
    write_passenger_spline(ctx, spline);
    ctx.db
        .game_taxi_flight_schedule()
        .insert(TaxiFlightSchedule {
            scheduled_id: character_guid,
            scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(TAXI_TICK_MICROS)),
        });
}

#[spacetimedb::reducer]
pub fn gw_arm_taxi_flight(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    arm_taxi_flight(ctx, character_guid);
    Ok(())
}

/// Advance one passenger from the authoritative clock. Missing live entities are retained for a
/// relog; malformed catalogue state cancels at the last authoritative position and never grants the
/// destination. Normal completion is the only branch that writes the exact destination.
#[spacetimedb::reducer]
pub fn advance_taxi_flight(ctx: &ReducerContext, schedule: TaxiFlightSchedule) {
    if ctx.sender() != ctx.database_identity() {
        return;
    }
    let character_guid = schedule.scheduled_id;
    let Some(mut flight) = ctx
        .db
        .game_active_taxi_flight()
        .character_guid()
        .find(character_guid)
    else {
        ctx.db
            .game_taxi_flight_schedule()
            .scheduled_id()
            .delete(character_guid);
        return;
    };
    let Some(mut player) = ctx.db.game_world_entity().guid().find(character_guid) else {
        return;
    };
    let Some(points) = ordered_route(ctx, flight.path_id) else {
        clear_flight(ctx, player);
        return;
    };
    let elapsed = ctx
        .timestamp
        .to_micros_since_unix_epoch()
        .saturating_sub(flight.started_micros);
    let Some(position) = route_position(&points, elapsed) else {
        clear_flight(ctx, player);
        return;
    };
    let Some(destination) = ctx
        .db
        .game_taxi_node()
        .id()
        .find(flight.destination_node_id)
    else {
        clear_flight(ctx, player);
        return;
    };
    if destination.map_id != player.map_id
        || points.iter().any(|point| point.map_id != player.map_id)
    {
        clear_flight(ctx, player);
        return;
    }
    if position.complete {
        land_flight(ctx, player, &destination, position.orientation);
        return;
    }

    let previous_cell = (player.grid_x, player.grid_y);
    let (grid_x, grid_y) = lyracore_shared::spatial::grid_cell(position.x, position.y);
    player.x = position.x;
    player.y = position.y;
    player.z = position.z;
    player.orientation = position.orientation;
    player.grid_x = grid_x;
    player.grid_y = grid_y;
    player.cell = lyracore_shared::spatial::grid_cell_id(grid_x, grid_y);
    player.movement_flags = TAXI_MOVEMENT_FORWARD;
    let crossed_point = flight.current_node_index != position.point_index as u32;
    flight.current_node_index = position.point_index as u32;
    let spline = should_refresh_spline(previous_cell, (grid_x, grid_y), crossed_point)
        .then(|| remaining_spline(&flight, &player, &points, position, elapsed));
    ctx.db.game_world_entity().guid().update(player);
    ctx.db
        .game_active_taxi_flight()
        .character_guid()
        .update(flight.clone());
    if let Some(spline) = spline {
        write_passenger_spline(ctx, spline);
    }
}

/// Delete an observed reply. Missing rows are an idempotent success; the character check prevents
/// a request id from acknowledging another character's mailbox entry.
#[spacetimedb::reducer]
pub fn gw_ack_taxi_reply(
    ctx: &ReducerContext,
    character_guid: u64,
    request_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let replies = ctx.db.game_taxi_service_reply();
    if replies
        .request_id()
        .find(request_id)
        .as_ref()
        .is_some_and(|reply| reply_belongs_to_character(reply, character_guid))
    {
        replies.request_id().delete(request_id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GameTaxiPath;

    struct FakeFlightEnd {
        world: LandingPosition,
        presentation: FlightPresentation,
        durable: LandingPosition,
        pending_instance_id: u64,
        zone_id: u32,
        spline: bool,
        schedule: bool,
        active: bool,
        spline_deletes: u8,
        schedule_deletes: u8,
        active_deletes: u8,
        pending_drops: u8,
    }

    impl FlightEndSink for FakeFlightEnd {
        fn project_world(&mut self, landing: Option<LandingPosition>) {
            if let Some(landing) = landing {
                self.world = landing;
            }
            self.presentation = cleared_presentation(self.presentation);
        }

        fn project_character(
            &mut self,
            landing: LandingPosition,
            instance_id: u64,
            zone_id: Option<u32>,
        ) {
            self.durable = landing;
            self.pending_instance_id = instance_id;
            if let Some(zone_id) = zone_id {
                self.zone_id = zone_id;
            }
        }

        fn drop_pending_motion(&mut self) {
            self.pending_drops += 1;
        }

        fn clear_spline(&mut self) {
            if std::mem::take(&mut self.spline) {
                self.spline_deletes += 1;
            }
        }

        fn clear_schedule(&mut self) {
            if std::mem::take(&mut self.schedule) {
                self.schedule_deletes += 1;
            }
        }

        fn take_active(&mut self) -> bool {
            if std::mem::take(&mut self.active) {
                self.active_deletes += 1;
                true
            } else {
                false
            }
        }
    }

    fn fake_end(position: LandingPosition) -> FakeFlightEnd {
        FakeFlightEnd {
            world: position,
            presentation: FlightPresentation {
                mount_display_id: 1147,
                unit_flags: 0x40 | lyracore_shared::constants::unit_flags::TAXI_FLIGHT,
                movement_flags: TAXI_MOVEMENT_FORWARD,
            },
            durable: position,
            pending_instance_id: 3,
            zone_id: 12,
            spline: true,
            schedule: true,
            active: true,
            spline_deletes: 0,
            schedule_deletes: 0,
            active_deletes: 0,
            pending_drops: 0,
        }
    }

    fn node(id: u32, client_node_id: u32, map_id: u32) -> GameTaxiNode {
        GameTaxiNode {
            id,
            client_node_id,
            map_id,
            x: 0.0,
            y: 0.0,
            z: 0.0,
            name: String::new(),
            mount_display_horde: 1,
            mount_display_alliance: 2,
        }
    }

    fn point(id: u32, path_id: u32, node_index: u32, map_id: u32) -> GameTaxiPathNode {
        GameTaxiPathNode {
            id,
            path_id,
            node_index,
            map_id,
            x: node_index as f32,
            y: 0.0,
            z: 0.0,
            flags: 0,
            delay_ms: 0,
        }
    }

    #[test]
    fn complete_same_map_geometry_is_supported() {
        let source = node(5_090_100, 255, 0);
        let destination = node(5_090_101, 256, 0);
        let points = vec![point(1, 99, 0, 0), point(2, 99, 1, 0)];
        assert!(route_is_supported(&source, &destination, &points));
    }

    #[test]
    fn incomplete_or_cross_map_geometry_is_not_advertised() {
        let source = node(10, 10, 0);
        let destination = node(20, 20, 0);
        assert!(!route_is_supported(
            &source,
            &destination,
            &[point(1, 70, 0, 0)]
        ));
        let mut delayed = vec![point(1, 70, 0, 0), point(2, 70, 1, 0)];
        delayed[0].delay_ms = 1;
        assert!(!route_is_supported(&source, &destination, &delayed));
        delayed[0].delay_ms = 0;
        delayed[1].flags = 1;
        assert!(!route_is_supported(&source, &destination, &delayed));
        assert!(!route_is_supported(
            &source,
            &destination,
            &[point(1, 70, 0, 0), point(2, 70, 2, 0)]
        ));
        assert!(!route_is_supported(
            &source,
            &destination,
            &[point(1, 70, 0, 0), point(2, 70, 1, 1)]
        ));
        assert!(!route_is_supported(
            &source,
            &node(20, 20, 1),
            &[point(1, 70, 0, 0), point(2, 70, 1, 0)]
        ));
    }

    #[test]
    fn storage_and_client_node_ids_are_distinct_contracts() {
        let source = node(5_090_100, 255, 0);
        let path = GameTaxiPath {
            id: 5_090_102,
            source_node_id: source.id,
            destination_node_id: 5_090_101,
            fare: 25,
        };
        assert_eq!(path.source_node_id, 5_090_100);
        assert_eq!(source.client_node_id, 255);
        assert_ne!(path.source_node_id, source.client_node_id);
    }

    #[test]
    fn interaction_and_source_resolution_boundaries_are_inclusive() {
        assert_eq!(
            squared_distance((0.0, 0.0, 0.0), (10.0, 0.0, 0.0)),
            TAXI_INTERACTION_RANGE_SQ
        );
        assert_eq!(TAXI_NODE_RESOLVE_RANGE_SQ, TAXI_INTERACTION_RANGE_SQ);
    }

    #[test]
    fn not_selectable_flight_masters_are_not_interactable() {
        assert_eq!(
            lyracore_shared::constants::unit_flags::NOT_SELECTABLE,
            0x0200_0000
        );
        assert!(flight_master_is_selectable(0));
        assert!(!flight_master_is_selectable(
            lyracore_shared::constants::unit_flags::NOT_SELECTABLE
        ));
    }

    #[test]
    fn first_discovery_inserts_and_repeated_discovery_is_idempotent() {
        assert!(should_insert_discovery(false));
        assert!(!should_insert_discovery(true));
    }

    #[test]
    fn arbitrary_overlap_never_reaps_young_unobserved_replies() {
        let now = 100_000_000;
        let rows: Vec<_> = (1..=10_000)
            .map(|id| (id, now - TAXI_REPLY_STALE_AFTER_MICROS + 1))
            .collect();
        assert!(stale_reply_ids(rows, now).is_empty());
    }

    #[test]
    fn stale_crash_leftovers_reap_at_the_fallback_boundary() {
        let now = 100_000_000;
        let rows = vec![
            (1, now - TAXI_REPLY_STALE_AFTER_MICROS - 1),
            (2, now - TAXI_REPLY_STALE_AFTER_MICROS),
            (3, now - TAXI_REPLY_STALE_AFTER_MICROS + 1),
            (4, now),
        ];
        assert_eq!(stale_reply_ids(rows, now), vec![1, 2]);
    }

    #[test]
    fn acknowledgement_is_operator_gated_character_checked_and_request_keyed() {
        let row = TaxiServiceReply {
            character_guid: 7,
            request_id: 99,
            operation: lyracore_shared::constants::taxi_protocol::REPLY_OPEN,
            npc_guid: 90,
            accepted: true,
            known: true,
            source_client_node_id: 255,
            available_client_node_ids: vec![255],
            refusal: String::new(),
            created_micros: 1,
            result_code: lyracore_shared::constants::taxi_protocol::ACTIVATE_OK,
        };
        assert!(reply_belongs_to_character(&row, 7));
        assert!(!reply_belongs_to_character(&row, 8));

        let ack = crate::test_scan::code_of(include_str!("taxi.rs"), "pub fn gw_ack_taxi_reply(");
        assert!(ack.contains("require_operator(ctx)"));
        assert!(ack.contains(".find(request_id)"));
        assert!(ack.contains("reply_belongs_to_character(reply, character_guid)"));
        assert!(ack.contains(".delete(request_id)"));
    }

    #[test]
    fn status_never_discovers_and_open_discovers_before_building_availability() {
        let source = include_str!("taxi.rs");
        let status = crate::test_scan::code_of(source, "pub fn gw_taxi_node_status(");
        assert!(
            !status.contains("discover("),
            "a status query must remain a persisted-state read"
        );

        let open = crate::test_scan::code_of(source, "pub fn gw_open_taxi(");
        let discovers = open.find("discover(").expect("open discovers its source");
        let builds = open
            .find("available_client_nodes(")
            .expect("open builds route availability");
        assert!(
            discovers < builds,
            "the source must become known before availability is constructed"
        );

        let resolver = crate::test_scan::code_of(source, "fn resolve_flight_master(");
        assert!(
            resolver.contains("acting_entity_by_guid"),
            "the gateway actor must resolve through the in-transit fence"
        );
    }

    #[test]
    fn every_activation_refusal_builds_a_committable_stable_reply() {
        use lyracore_shared::constants::taxi_protocol as wire;
        let cases = [
            (
                TaxiGateDenied::PlayerMissing,
                wire::ACTIVATE_UNSPECIFIED_SERVER_ERROR,
            ),
            (TaxiGateDenied::PlayerDead, wire::ACTIVATE_PLAYER_BUSY),
            (
                TaxiGateDenied::FlightMasterMissing,
                wire::ACTIVATE_NO_VENDOR_NEARBY,
            ),
            (
                TaxiGateDenied::FlightMasterDead,
                wire::ACTIVATE_NO_VENDOR_NEARBY,
            ),
            (
                TaxiGateDenied::NotFlightMaster,
                wire::ACTIVATE_NO_VENDOR_NEARBY,
            ),
            (
                TaxiGateDenied::DifferentPartition,
                wire::ACTIVATE_TOO_FAR_AWAY,
            ),
            (TaxiGateDenied::OutOfRange, wire::ACTIVATE_TOO_FAR_AWAY),
            (TaxiGateDenied::Hostile, wire::ACTIVATE_NO_VENDOR_NEARBY),
            (
                TaxiGateDenied::NotInteractable,
                wire::ACTIVATE_NO_VENDOR_NEARBY,
            ),
            (TaxiGateDenied::NoSourceNode, wire::ACTIVATE_NO_SUCH_PATH),
            (
                TaxiGateDenied::UnsupportedInstance,
                wire::ACTIVATE_PLAYER_BUSY,
            ),
            (TaxiGateDenied::PlayerBusy, wire::ACTIVATE_PLAYER_BUSY),
            (
                TaxiGateDenied::PlayerAlreadyMounted,
                wire::ACTIVATE_PLAYER_ALREADY_MOUNTED,
            ),
            (TaxiGateDenied::PlayerMoving, wire::ACTIVATE_PLAYER_MOVING),
            (
                TaxiGateDenied::PlayerShapeShifted,
                wire::ACTIVATE_PLAYER_SHAPE_SHIFTED,
            ),
            (
                TaxiGateDenied::PlayerNotStanding,
                wire::ACTIVATE_NOT_STANDING,
            ),
            (TaxiGateDenied::SourceMismatch, wire::ACTIVATE_NOT_VISITED),
            (
                TaxiGateDenied::DestinationMissing,
                wire::ACTIVATE_NO_SUCH_PATH,
            ),
            (
                TaxiGateDenied::DestinationUnknown,
                wire::ACTIVATE_NOT_VISITED,
            ),
            (TaxiGateDenied::SameNode, wire::ACTIVATE_SAME_NODE),
            (TaxiGateDenied::NoDirectPath, wire::ACTIVATE_NO_SUCH_PATH),
            (TaxiGateDenied::MissingMount, wire::ACTIVATE_NO_SUCH_PATH),
            (
                TaxiGateDenied::NotEnoughMoney,
                wire::ACTIVATE_NOT_ENOUGH_MONEY,
            ),
        ];
        for (denied, expected) in cases {
            let reply = activation_reply(7, 11, 90, 255, 1234, Err(denied));
            assert!(!reply.accepted, "{denied:?}");
            assert_eq!(reply.result_code, expected, "{denied:?}");
            assert!(!reply.refusal.is_empty(), "{denied:?}");
            assert_eq!(reply.operation, wire::REPLY_ACTIVATE);
            assert_eq!((reply.character_guid, reply.request_id), (7, 11));
        }
        let accepted = activation_reply(7, 11, 90, 255, 1234, Ok(()));
        assert!(accepted.accepted);
        assert_eq!(accepted.result_code, wire::ACTIVATE_OK);
        assert!(accepted.refusal.is_empty());
    }

    #[test]
    fn fare_debit_is_exact_and_never_wraps() {
        assert_eq!(debit_fare(100, 25), Ok(75));
        assert_eq!(debit_fare(25, 25), Ok(0));
        assert_eq!(debit_fare(24, 25), Err(TaxiGateDenied::NotEnoughMoney));
    }

    #[test]
    fn faction_mount_selection_is_deterministic_and_preserves_missing_zero() {
        let mut source = node(1, 1, 0);
        source.mount_display_horde = 295;
        source.mount_display_alliance = 1147;
        assert_eq!(faction_mount_display(2, &source), 295); // Orc
        assert_eq!(faction_mount_display(1, &source), 1147); // Human
        source.mount_display_horde = 0;
        assert_eq!(faction_mount_display(6, &source), 0); // rejected by activation
    }

    #[test]
    fn activation_state_machine_mutates_success_and_charges_at_most_once() {
        let mut state = ActivationPlayerState {
            has_active_flight: false,
            money: 100,
            mount_display_id: 0,
            movement_flags: 0,
            stance: crate::spell::STANCE_BATTLE,
            stand_state: 0,
            unit_flags: 0,
        };

        assert_eq!(state.activate(25, 1147), Ok(()));
        assert!(state.has_active_flight);
        assert_eq!(state.money, 75);
        assert_eq!(state.mount_display_id, 1147);
        assert_ne!(
            state.unit_flags & lyracore_shared::constants::unit_flags::TAXI_FLIGHT,
            0
        );

        let after_first = state;
        assert_eq!(state.activate(25, 1147), Err(TaxiGateDenied::PlayerBusy));
        assert_eq!(
            state, after_first,
            "a serialized retry must not charge twice"
        );
    }

    #[test]
    fn activation_state_machine_insufficient_funds_has_no_mutation() {
        let mut state = ActivationPlayerState {
            has_active_flight: false,
            money: 24,
            mount_display_id: 0,
            movement_flags: 0,
            stance: crate::spell::STANCE_DEFENSIVE,
            stand_state: 0,
            unit_flags: 0,
        };
        let before = state;

        assert_eq!(
            state.activate(25, 1147),
            Err(TaxiGateDenied::NotEnoughMoney)
        );
        assert_eq!(state, before);
    }

    #[test]
    fn authoritative_route_clock_covers_start_progress_and_exact_landing() {
        let points = vec![point(1, 99, 0, 0), point(2, 99, 1, 0), point(3, 99, 2, 0)];
        let first_leg = segment_micros(&points[0], &points[1]);
        let total = route_duration_micros(&points).unwrap();

        let start = route_position(&points, 0).unwrap();
        assert_eq!(
            (start.point_index, start.x, start.complete),
            (0, 0.0, false)
        );

        let middle = route_position(&points, first_leg / 2).unwrap();
        assert_eq!(middle.point_index, 0);
        assert!((middle.x - 0.5).abs() < 0.01);
        assert!(!middle.complete);

        let landing = route_position(&points, total).unwrap();
        assert_eq!((landing.x, landing.y, landing.z), (2.0, 0.0, 0.0));
        assert!(landing.complete);
        assert_eq!(route_position(&points, total + 10_000_000), Some(landing));

        let mut destination = node(20, 256, 0);
        (destination.x, destination.y, destination.z) = (-9443.5, 58.25, 56.75);
        let durable = exact_landing(&destination, 1.25);
        assert_eq!(
            (
                durable.map_id,
                durable.x,
                durable.y,
                durable.z,
                durable.orientation
            ),
            (0, -9443.5, 58.25, 56.75, 1.25)
        );
        assert_eq!(
            durable.cell,
            lyracore_shared::spatial::grid_cell_id(durable.grid_x, durable.grid_y)
        );
    }

    #[test]
    fn route_time_is_monotonic_and_does_not_accumulate_tick_deltas() {
        let points = vec![point(1, 99, 0, 0), point(2, 99, 1, 0)];
        let total = route_duration_micros(&points).unwrap();
        let samples: Vec<f32> = [0, total / 4, total / 2, total * 3 / 4, total]
            .into_iter()
            .map(|elapsed| route_position(&points, elapsed).unwrap().x)
            .collect();
        assert!(samples.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(samples.last().copied(), Some(1.0));
    }

    #[test]
    fn action_and_movement_policy_is_executable() {
        assert_eq!(action_gate(true), Err("PLAYER_IN_TAXI_FLIGHT".to_string()));
        assert_eq!(action_gate(false), Ok(()));
    }

    #[test]
    fn presentation_cleanup_is_idempotent_and_cell_crossings_refresh_the_spline() {
        let active = FlightPresentation {
            mount_display_id: 1147,
            unit_flags: 0x55 | lyracore_shared::constants::unit_flags::TAXI_FLIGHT,
            movement_flags: TAXI_MOVEMENT_FORWARD,
        };
        let cleared = cleared_presentation(active);
        assert_eq!(cleared_presentation(cleared), cleared);
        assert_eq!(cleared.mount_display_id, 0);
        assert_eq!(cleared.movement_flags, 0);
        assert_eq!(
            cleared.unit_flags & lyracore_shared::constants::unit_flags::TAXI_FLIGHT,
            0
        );
        assert!(should_refresh_spline((1, 1), (2, 1), false));
        assert!(should_refresh_spline((1, 1), (1, 1), true));
        assert!(!should_refresh_spline((1, 1), (1, 1), false));
    }

    #[test]
    fn authority_sink_projects_exact_landing_and_cleans_each_row_once() {
        let old = LandingPosition {
            map_id: 0,
            x: -1.0,
            y: -2.0,
            z: -3.0,
            orientation: 0.0,
            grid_x: 1,
            grid_y: 2,
            cell: lyracore_shared::spatial::grid_cell_id(1, 2),
        };
        let mut destination = node(20, 256, 0);
        (destination.x, destination.y, destination.z) = (-9443.5, 58.25, 56.75);
        let landing = exact_landing(&destination, 1.25);
        let plan = FlightEndPlan {
            landing: Some(landing),
            instance_id: 7,
            zone_id: Some(40),
        };
        let mut sink = fake_end(old);

        apply_flight_end(&mut sink, plan);
        apply_flight_end(&mut sink, plan);

        assert_eq!(sink.world, landing);
        assert_eq!(sink.durable, landing);
        assert_eq!((sink.pending_instance_id, sink.zone_id), (7, 40));
        assert_eq!(sink.presentation, cleared_presentation(sink.presentation));
        assert_eq!(
            (
                sink.spline_deletes,
                sink.schedule_deletes,
                sink.active_deletes
            ),
            (1, 1, 1)
        );
        assert_eq!(sink.pending_drops, 1);
    }

    #[test]
    fn malformed_cancellation_preserves_last_position_and_never_grants_destination() {
        let last = LandingPosition {
            map_id: 0,
            x: -9500.0,
            y: 25.0,
            z: 60.0,
            orientation: 0.75,
            grid_x: 2,
            grid_y: 3,
            cell: lyracore_shared::spatial::grid_cell_id(2, 3),
        };
        let mut sink = fake_end(last);
        apply_flight_end(
            &mut sink,
            FlightEndPlan {
                landing: None,
                instance_id: 99,
                zone_id: Some(999),
            },
        );

        assert_eq!(sink.world, last);
        assert_eq!(sink.durable, last);
        assert_eq!((sink.pending_instance_id, sink.zone_id), (3, 12));
        assert_eq!(sink.presentation.mount_display_id, 0);
        assert_eq!(
            (
                sink.spline_deletes,
                sink.schedule_deletes,
                sink.active_deletes
            ),
            (1, 1, 1)
        );
        assert_eq!(sink.pending_drops, 0);
    }

    #[test]
    fn active_flight_is_explicitly_not_transported() {
        assert!(crate::transfer::NOT_TRANSPORTED.contains(&"game_active_taxi_flight"));
    }
}
