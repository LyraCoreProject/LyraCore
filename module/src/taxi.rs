use lyracore_shared::movement_flags;
use lyracore_shared::movement_path::{
    catmull_rom, catmull_rom_slopes, catmull_rom_weights, extrapolated_start,
};
use spacetimedb::{reducer, table, ReducerContext, ScheduleAt, Table, TimeDuration};

use crate::world::MoveClock;
use crate::{game_character, game_world_entity, WorldEntity};

pub(crate) mod catalogue;
pub(crate) use catalogue::{
    game_taxi_node, game_taxi_path, game_taxi_path_node, GameTaxiNode, GameTaxiPath,
    GameTaxiPathNode,
};

#[reducer]
pub fn restore_taxi_fixture(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::seed::seed_taxi_fixture(ctx);
    Ok(())
}

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
    #[default(0.0f32)]
    pub origin_x: f32,
    #[default(0.0f32)]
    pub origin_y: f32,
    #[default(0.0f32)]
    pub origin_z: f32,
    #[default(false)]
    pub origin_recorded: bool,
    #[default(1u32)]
    pub first_destination_node_index: u32,
    #[default(0u32)]
    pub current_leg_index: u32,
}

/// A leg after the first of a multi-hop flight; leg 0 is the flight's `path_id`.
#[table(
    accessor = game_taxi_flight_leg,
    index(accessor = by_character, btree(columns = [character_guid]))
)]
pub struct TaxiFlightLeg {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub character_guid: u64,
    pub leg_index: u32,
    pub path_id: u32,
}

#[table(accessor = game_taxi_flight_schedule, scheduled(advance_taxi_flight))]
pub struct TaxiFlightSchedule {
    #[primary_key]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}

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
    pub points: Vec<f32>,
    pub duration_ms: u32,
    pub spline_id: u32,
}

crate::character_owned!(delete, fn sweep_delete_game_active_taxi_flight(ctx, character_guid) {
    ctx.db.game_active_taxi_flight().character_guid().delete(character_guid);
});
// A flight cannot cross Shards; its route and catalogue belong to the current hosted map.
crate::character_owned!(not_transported, fn sweep_transfer_game_active_taxi_flight());

crate::character_owned!(delete, fn sweep_delete_game_taxi_flight_leg(ctx, character_guid) {
    delete_onward_legs(ctx, character_guid);
});
crate::character_owned!(not_transported, fn sweep_transfer_game_taxi_flight_leg());

fn delete_onward_legs(ctx: &ReducerContext, character_guid: u64) {
    let legs = ctx.db.game_taxi_flight_leg();
    for leg in legs
        .by_character()
        .filter(&character_guid)
        .collect::<Vec<_>>()
    {
        legs.id().delete(leg.id);
    }
}

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

crate::character_owned!(transfer, fn sweep_transfer_game_character_taxi_node(ctx, character_guid, io) {
    table = game_character_taxi_node,
    by = by_character,
    remint = id,
});

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
    PlayerLostControl,
    FlightMasterMissing,
    FlightMasterDead,
    FlightMasterInCombat,
    NotFlightMaster,
    DifferentPartition,
    OutOfRange,
    Hostile,
    NotInteractable,
    NoSourceNode,
    Flying,
    PlayerBusy,
    NoClientControl,
    UnknownNode,
    PlayerAlreadyMounted,
    PlayerShapeShifted,
    NodeWithoutPosition,
    SourceTooFar,
    NoPath,
    UnsupportedRoute,
    MissingMount,
    NotEnoughMoney,
}

impl TaxiGateDenied {
    fn message(self) -> &'static str {
        match self {
            Self::PlayerMissing => "player not in world",
            Self::PlayerDead => "dead players cannot use a taxi",
            Self::PlayerLostControl => "player has lost control",
            Self::FlightMasterMissing => "no such flight master",
            Self::FlightMasterDead => "flight master is dead",
            Self::FlightMasterInCombat => "flight master is in combat",
            Self::NotFlightMaster => "target is not a flight master",
            Self::DifferentPartition => "flight master is in another world partition",
            Self::OutOfRange => "flight master is out of range",
            Self::Hostile => "flight master refuses interaction",
            Self::NotInteractable => "flight master is not interactable",
            Self::NoSourceNode => "flight master has no taxi node",
            Self::Flying => "player is in a taxi flight",
            Self::PlayerBusy => "player is busy",
            Self::NoClientControl => "player has no client control",
            Self::UnknownNode => "taxi node does not exist or has not been visited",
            Self::PlayerAlreadyMounted => "player is already mounted",
            Self::PlayerShapeShifted => "player is shape-shifted",
            Self::NodeWithoutPosition => "source taxi node has no position",
            Self::SourceTooFar => "source taxi node is too far away",
            Self::NoPath => "no taxi path between the nodes",
            Self::UnsupportedRoute => "a leg has no route the Module can fly",
            Self::MissingMount => "taxi node has no usable faction mount",
            Self::NotEnoughMoney => "not enough money for taxi fare",
        }
    }

    /// The `SMSG_ACTIVATETAXIREPLY` status vmangos sends for this denial. `None` is the silence of
    /// `CanInteractWithNPC`, a flying Character, no client control, an unknown node and no path.
    fn activation_status(self) -> Option<u8> {
        use lyracore_shared::constants::taxi_protocol as wire;
        match self {
            Self::PlayerMissing
            | Self::PlayerDead
            | Self::PlayerLostControl
            | Self::FlightMasterMissing
            | Self::FlightMasterDead
            | Self::FlightMasterInCombat
            | Self::NotFlightMaster
            | Self::DifferentPartition
            | Self::OutOfRange
            | Self::Hostile
            | Self::NotInteractable
            | Self::Flying
            | Self::NoClientControl
            | Self::UnknownNode
            | Self::NoPath => None,
            Self::PlayerBusy => Some(wire::ACTIVATE_PLAYER_BUSY),
            Self::PlayerAlreadyMounted => Some(wire::ACTIVATE_PLAYER_ALREADY_MOUNTED),
            Self::PlayerShapeShifted => Some(wire::ACTIVATE_PLAYER_SHAPE_SHIFTED),
            Self::NoSourceNode | Self::UnsupportedRoute => Some(wire::ACTIVATE_NO_SUCH_PATH),
            Self::SourceTooFar => Some(wire::ACTIVATE_TOO_FAR_AWAY),
            Self::NodeWithoutPosition | Self::MissingMount => {
                Some(wire::ACTIVATE_UNSPECIFIED_SERVER_ERROR)
            }
            Self::NotEnoughMoney => Some(wire::ACTIVATE_NOT_ENOUGH_MONEY),
        }
    }

    fn is_silent(self) -> bool {
        self.activation_status().is_none()
    }

    /// The status a recorded service reply carries; a silent denial records the server error.
    fn activate_result_code(self) -> u8 {
        self.activation_status()
            .unwrap_or(lyracore_shared::constants::taxi_protocol::ACTIVATE_UNSPECIFIED_SERVER_ERROR)
    }
}

fn squared_distance(a: (f32, f32, f32), b: (f32, f32, f32)) -> f32 {
    let (dx, dy, dz) = (b.0 - a.0, b.1 - a.1, b.2 - a.2);
    dx * dx + dy * dy + dz * dz
}

fn flight_master_gate(player: &WorldEntity, npc: &WorldEntity) -> Result<(), TaxiGateDenied> {
    if npc.dead || npc.health == 0 {
        return Err(TaxiGateDenied::FlightMasterDead);
    }
    if npc.is_player() || npc.npc_flags & lyracore_shared::constants::npc_flags::TAXI == 0 {
        return Err(TaxiGateDenied::NotFlightMaster);
    }
    if npc.unit_flags & lyracore_shared::constants::unit_flags::IN_COMBAT != 0 {
        return Err(TaxiGateDenied::FlightMasterInCombat);
    }
    if npc.unit_flags & lyracore_shared::constants::unit_flags::NOT_SELECTABLE != 0 {
        return Err(TaxiGateDenied::NotInteractable);
    }
    if npc.map_id != player.map_id || npc.instance_id != player.instance_id {
        return Err(TaxiGateDenied::DifferentPartition);
    }
    if crate::helpers::dist_sq(player, npc) > crate::npc_interaction::INTERACTION_RANGE_SQ {
        return Err(TaxiGateDenied::OutOfRange);
    }
    Ok(())
}

/// vmangos `ObjectMgr::GetNearestTaxiNode`: the nearest node on the creature's map that is in the
/// taxi network and has a mount for the Character's team, however far; a tie goes to the lower id.
fn nearest_taxi_node(
    npc: &WorldEntity,
    race: u8,
    nodes: impl Iterator<Item = GameTaxiNode>,
) -> Option<GameTaxiNode> {
    nodes
        .filter(|node| {
            node.map_id == npc.map_id && node.in_taxi_network && has_own_mount(race, node)
        })
        .map(|node| {
            (
                squared_distance((npc.x, npc.y, npc.z), (node.x, node.y, node.z)),
                node,
            )
        })
        .min_by(|(da, a), (db, b)| da.total_cmp(db).then_with(|| a.id.cmp(&b.id)))
        .map(|(_, node)| node)
}

/// The Character and the flight master it can interact with (vmangos `Player::CanInteractWithNPC`).
fn interacting_flight_master(
    ctx: &ReducerContext,
    character_guid: u64,
    npc_guid: u64,
) -> Result<(WorldEntity, WorldEntity), TaxiGateDenied> {
    let player = crate::helpers::acting_entity_by_guid(ctx, character_guid)
        .ok_or(TaxiGateDenied::PlayerMissing)?;
    if player.dead || player.health == 0 {
        return Err(TaxiGateDenied::PlayerDead);
    }
    if crate::spell::is_action_blocked(ctx, player.guid) {
        return Err(TaxiGateDenied::PlayerLostControl);
    }
    let npc = ctx
        .db
        .game_world_entity()
        .guid()
        .find(npc_guid)
        .ok_or(TaxiGateDenied::FlightMasterMissing)?;
    flight_master_gate(&player, &npc)?;
    if crate::reputation::npc_refuses_interaction(ctx, &npc, &player) {
        return Err(TaxiGateDenied::Hostile);
    }
    Ok((player, npc))
}

fn resolve_flight_master(
    ctx: &ReducerContext,
    character_guid: u64,
    npc_guid: u64,
) -> Result<(WorldEntity, GameTaxiNode), TaxiGateDenied> {
    let (player, npc) = interacting_flight_master(ctx, character_guid, npc_guid)?;
    let source = nearest_taxi_node(&npc, player.race(), ctx.db.game_taxi_node().iter())
        .ok_or(TaxiGateDenied::NoSourceNode)?;
    Ok((player, source))
}

fn has_active_flight(ctx: &ReducerContext, character_guid: u64) -> bool {
    ctx.db
        .game_active_taxi_flight()
        .character_guid()
        .find(character_guid)
        .is_some()
}

/// The packet's source node must lie near the Character. vmangos compares a squared distance with
/// `(2 * INTERACTION_DISTANCE)^3`, and `INTERACTION_DISTANCE` is 5 yards.
const SOURCE_NODE_REACH_SQ: f32 = 1000.0;

fn source_node_in_reach(player: &WorldEntity, source: &GameTaxiNode) -> Result<(), TaxiGateDenied> {
    let position = (source.x, source.y, source.z);
    if position == (0.0, 0.0, 0.0) {
        return Err(TaxiGateDenied::NodeWithoutPosition);
    }
    if source.map_id != player.map_id
        || squared_distance((player.x, player.y, player.z), position) > SOURCE_NODE_REACH_SQ
    {
        return Err(TaxiGateDenied::SourceTooFar);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ActivationPlayerState {
    has_active_flight: bool,
    money: u32,
    mount_display_id: u32,
    form_blocks_mounting: bool,
    casting: bool,
    unit_flags: u32,
}

impl ActivationPlayerState {
    fn from_world(ctx: &ReducerContext, player: &WorldEntity) -> Self {
        Self {
            has_active_flight: has_active_flight(ctx, player.guid),
            money: player.money,
            mount_display_id: player.mount_display_id,
            form_blocks_mounting: crate::form::blocks_mounting(ctx, player.guid),
            casting: crate::spell::is_non_melee_spell_casting(ctx, player.guid),
            unit_flags: player.unit_flags,
        }
    }

    /// vmangos checks `CanInteractWithNPC` and the node count first.
    fn validate_can_act(self) -> Result<(), TaxiGateDenied> {
        use lyracore_shared::constants::unit_flags;
        if self.has_active_flight {
            return Err(TaxiGateDenied::Flying);
        }
        if self.unit_flags & unit_flags::IN_COMBAT != 0 {
            return Err(TaxiGateDenied::PlayerBusy);
        }
        if self.unit_flags & unit_flags::REMOVE_CLIENT_CONTROL != 0 {
            return Err(TaxiGateDenied::NoClientControl);
        }
        Ok(())
    }

    fn validate_can_mount(self) -> Result<(), TaxiGateDenied> {
        if self.mount_display_id != 0 {
            return Err(TaxiGateDenied::PlayerAlreadyMounted);
        }
        if self.form_blocks_mounting {
            return Err(TaxiGateDenied::PlayerShapeShifted);
        }
        if self.casting {
            return Err(TaxiGateDenied::PlayerBusy);
        }
        Ok(())
    }

    fn activate(&mut self, leg_fares: &[u32], mount_display_id: u32) -> Result<(), TaxiGateDenied> {
        self.validate_can_act()?;
        if mount_display_id == 0 {
            return Err(TaxiGateDenied::MissingMount);
        }
        let remaining_money = debit_first_leg(self.money, leg_fares)?;

        self.has_active_flight = true;
        self.money = remaining_money;
        self.mount_display_id = mount_display_id;
        self.unit_flags |= lyracore_shared::constants::unit_flags::TAXI_FLIGHT
            | lyracore_shared::constants::unit_flags::REMOVE_CLIENT_CONTROL;
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

fn has_own_mount(race: u8, node: &GameTaxiNode) -> bool {
    if lyracore_shared::faction::team_for_race(race) == lyracore_shared::faction::TEAM_HORDE {
        node.own_mount_horde
    } else {
        node.own_mount_alliance
    }
}

/// vmangos `ActivateTaxiPathTo`: the money must cover every leg; only the first is paid now.
fn debit_first_leg(money: u32, leg_fares: &[u32]) -> Result<u32, TaxiGateDenied> {
    let route_fare = leg_fares
        .iter()
        .fold(0u32, |total, &fare| total.saturating_add(fare));
    if money < route_fare {
        return Err(TaxiGateDenied::NotEnoughMoney);
    }
    Ok(money - leg_fares.first().copied().unwrap_or_default())
}

/// Each leg's path; vmangos is silent at a missing path, so that outranks an unflyable leg.
fn route_legs(
    ctx: &ReducerContext,
    nodes: &[GameTaxiNode],
) -> Result<Vec<GameTaxiPath>, TaxiGateDenied> {
    let candidates: Vec<Vec<GameTaxiPath>> = nodes
        .windows(2)
        .map(|leg| {
            ctx.db
                .game_taxi_path()
                .by_source()
                .filter(&leg[0].id)
                .filter(|path| path.destination_node_id == leg[1].id)
                .collect()
        })
        .collect();
    if candidates.iter().any(Vec::is_empty) {
        return Err(TaxiGateDenied::NoPath);
    }
    nodes
        .windows(2)
        .zip(candidates)
        .map(|(leg, paths)| {
            let [path] = <[GameTaxiPath; 1]>::try_from(paths)
                .map_err(|_| TaxiGateDenied::UnsupportedRoute)?;
            ordered_route(ctx, path.id)
                .is_some_and(|points| route_is_supported(&leg[0], &leg[1], &points))
                .then_some(path)
                .ok_or(TaxiGateDenied::UnsupportedRoute)
        })
        .collect()
}

fn is_known(ctx: &ReducerContext, character_guid: u64, node_id: u32) -> bool {
    ctx.db
        .game_character_taxi_node()
        .by_character()
        .filter(&character_guid)
        .any(|row| row.node_id == node_id)
}

/// Teaches the node; false when the Character already knew it.
pub(crate) fn discover(ctx: &ReducerContext, character_guid: u64, node_id: u32) -> bool {
    if is_known(ctx, character_guid, node_id) {
        return false;
    }
    ctx.db.game_character_taxi_node().insert(CharacterTaxiNode {
        id: 0,
        character_guid,
        node_id,
    });
    true
}

/// vmangos `SendLearnNewTaxiNode`: teaches the flight master's node; false when known or absent.
pub(crate) fn learn_nearest_node(
    ctx: &ReducerContext,
    character_guid: u64,
    npc: &WorldEntity,
) -> bool {
    let Some(player) = crate::helpers::acting_entity_by_guid(ctx, character_guid) else {
        return false;
    };
    nearest_taxi_node(npc, player.race(), ctx.db.game_taxi_node().iter())
        .is_some_and(|node| discover(ctx, character_guid, node.id))
}

/// Whether `points`, in node order, form one whole route: contiguous, without delays or flags.
fn is_complete_route(points: &[GameTaxiPathNode]) -> bool {
    points.len() >= 2
        && points.iter().enumerate().all(|(index, point)| {
            point.node_index == index as u32 && point.delay_ms == 0 && point.flags == 0
        })
}

fn route_is_supported(
    source: &GameTaxiNode,
    destination: &GameTaxiNode,
    points: &[GameTaxiPathNode],
) -> bool {
    source.map_id == destination.map_id
        && is_complete_route(points)
        && points.iter().all(|point| point.map_id == source.map_id)
}

const TAXI_SPEED_YARDS_PER_SECOND: f32 = 32.0;
pub(crate) const TAXI_TICK_MICROS: i64 = 100_000;

#[derive(Clone, Copy, Debug, PartialEq)]
struct RoutePosition {
    point_index: usize,
    x: f32,
    y: f32,
    z: f32,
    orientation: f32,
    complete: bool,
}

/// A flight's points in order, with the index in `points` of each leg's last point but the last.
#[derive(Default)]
struct RoutePoints {
    points: Vec<GameTaxiPathNode>,
    leg_ends: Vec<usize>,
}

impl RoutePoints {
    /// vmangos default joints: a leg ends at its next-to-last point, the next starts at its second.
    fn join(legs: Vec<Vec<GameTaxiPathNode>>) -> Self {
        let last = legs.len().saturating_sub(1);
        let mut joined = Self::default();
        for (index, leg) in legs.into_iter().enumerate() {
            let taken = if index == last {
                leg.len()
            } else {
                leg.len().saturating_sub(1)
            };
            joined
                .points
                .extend(leg.into_iter().take(taken).skip(usize::from(index > 0)));
            if index < last {
                joined.leg_ends.push(joined.points.len().saturating_sub(1));
            }
        }
        joined
    }

    /// The leg the flyer is on while it flies from the point at `point_index` to the next.
    fn leg_at(&self, point_index: usize) -> u32 {
        self.leg_ends
            .iter()
            .filter(|&&end| point_index >= end)
            .count() as u32
    }
}

/// The route as flown: the launch origin, then the points from `first_destination_node_index`.
fn flight_route(flight: &ActiveTaxiFlight, route: RoutePoints) -> RoutePoints {
    let Some(mut origin) = route.points.first().cloned() else {
        return RoutePoints::default();
    };
    if flight.origin_recorded {
        origin.x = flight.origin_x;
        origin.y = flight.origin_y;
        origin.z = flight.origin_z;
    }
    let skip = flight.first_destination_node_index as usize;
    RoutePoints {
        points: std::iter::once(origin)
            .chain(route.points.into_iter().skip(skip))
            .collect(),
        leg_ends: route
            .leg_ends
            .into_iter()
            .map(|end| (end + 1).saturating_sub(skip))
            .collect(),
    }
}

fn resume_node(points: &[GameTaxiPathNode], character: &WorldEntity) -> u32 {
    // The Reference uses a strict bracket and falls back to DBC node 0.
    points
        .windows(2)
        .enumerate()
        .find_map(|(index, pair)| {
            let next = &pair[1];
            if next.map_id != character.map_id {
                return None;
            }
            let previous = &pair[0];
            let dot = (character.x - previous.x) * (character.x - next.x)
                + (character.y - previous.y) * (character.y - next.y)
                + (character.z - previous.z) * (character.z - next.z);
            (dot < 0.0).then_some(index as u32 + 1)
        })
        .unwrap_or(0)
}

fn spline_controls(points: &[GameTaxiPathNode], index: usize) -> [[f32; 3]; 4] {
    let coords = |point: &GameTaxiPathNode| [point.x, point.y, point.z];
    let current = coords(&points[index]);
    let next = coords(&points[index + 1]);
    let prior = if index == 0 {
        std::array::from_fn(|axis| current[axis] - (next[axis] - current[axis]))
    } else {
        coords(&points[index - 1])
    };
    let after = points.get(index + 2).map_or(next, coords);
    [prior, current, next, after]
}

fn route_timestamps_micros(points: &[GameTaxiPathNode]) -> Option<Vec<i64>> {
    (points.len() >= 2).then(|| {
        // vmangos starts at 1 ms and truncates the clock after each float segment duration.
        let mut elapsed_ms = 1i64;
        let mut times = vec![0];
        for index in 0..points.len() - 1 {
            let controls = spline_controls(points, index);
            let mut previous = controls[1];
            let mut length = 0.0f64;
            for step in 1..=3 {
                let current = catmull_rom(&controls, catmull_rom_weights(step as f32 / 3.0));
                let delta: [f32; 3] = std::array::from_fn(|axis| current[axis] - previous[axis]);
                length += f64::from(
                    delta
                        .iter()
                        .map(|component| component * component)
                        .sum::<f32>()
                        .sqrt(),
                );
                previous = current;
            }
            elapsed_ms = (elapsed_ms as f32
                + (length as f32) * (1_000.0 / TAXI_SPEED_YARDS_PER_SECOND))
                as i64;
            times.push(elapsed_ms.saturating_mul(1_000));
        }
        times
    })
}

fn route_duration_micros(points: &[GameTaxiPathNode]) -> Option<i64> {
    route_timestamps_micros(points).and_then(|times| times.last().copied())
}

fn route_position(points: &[GameTaxiPathNode], elapsed_micros: i64) -> Option<RoutePosition> {
    if let [point] = points {
        return Some(RoutePosition {
            point_index: 0,
            x: point.x,
            y: point.y,
            z: point.z,
            orientation: 0.0,
            complete: true,
        });
    }
    let times = route_timestamps_micros(points)?;
    let total = *times.last()?;
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

    for index in 0..points.len() - 1 {
        if elapsed < times[index + 1] {
            let duration = times[index + 1] - times[index];
            let t = ((elapsed - times[index]) as f32 / duration as f32).clamp(0.0, 1.0);
            let controls = spline_controls(points, index);
            let position = catmull_rom(&controls, catmull_rom_weights(t));
            let direction = catmull_rom(&controls, catmull_rom_slopes(t));
            return Some(RoutePosition {
                point_index: index,
                x: position[0],
                y: position[1],
                z: position[2],
                orientation: direction[1].atan2(direction[0]),
                complete: false,
            });
        }
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
    is_complete_route(&points).then_some(points)
}

/// The path of each leg of `flight`, in route order.
fn leg_paths(ctx: &ReducerContext, flight: &ActiveTaxiFlight) -> Vec<u32> {
    let mut onward: Vec<_> = ctx
        .db
        .game_taxi_flight_leg()
        .by_character()
        .filter(&flight.character_guid)
        .collect();
    onward.sort_by_key(|leg| leg.leg_index);
    std::iter::once(flight.path_id)
        .chain(onward.into_iter().map(|leg| leg.path_id))
        .collect()
}

fn joined_route(ctx: &ReducerContext, legs: &[u32]) -> Option<RoutePoints> {
    legs.iter()
        .map(|&path_id| ordered_route(ctx, path_id))
        .collect::<Option<Vec<_>>>()
        .map(RoutePoints::join)
}

/// An active flight's route and destination while both still lie on the flyer's map.
struct FlightRoute {
    legs: Vec<u32>,
    route: RoutePoints,
    destination: GameTaxiNode,
}

impl FlightRoute {
    fn load(ctx: &ReducerContext, flight: &ActiveTaxiFlight, flyer: &WorldEntity) -> Option<Self> {
        let legs = leg_paths(ctx, flight);
        let route = joined_route(ctx, &legs)?;
        let destination = ctx
            .db
            .game_taxi_node()
            .id()
            .find(flight.destination_node_id)?;
        (destination.map_id == flyer.map_id
            && route
                .points
                .iter()
                .all(|point| point.map_id == flyer.map_id))
        .then_some(Self {
            legs,
            route,
            destination,
        })
    }
}

/// vmangos `FlightPathMovementGenerator::Update` pays each leg as the flyer enters it, and
/// `ModifyMoney` stops at zero.
fn pay_entered_legs(
    ctx: &ReducerContext,
    legs: &[u32],
    flight: &mut ActiveTaxiFlight,
    player: &mut WorldEntity,
    leg: u32,
) {
    let paid = flight.current_leg_index as usize;
    for &path_id in legs.iter().take(leg as usize + 1).skip(paid + 1) {
        let fare = ctx
            .db
            .game_taxi_path()
            .id()
            .find(path_id)
            .map_or(0, |path| path.fare);
        player.money = player.money.saturating_sub(fare);
    }
    flight.current_leg_index = flight.current_leg_index.max(leg);
}

pub(crate) fn current_create_spline(
    ctx: &ReducerContext,
    character_guid: u64,
) -> Option<(lyracore_protocol::codec::CreateSpline, [f32; 4])> {
    use lyracore_protocol::messages::vanilla::Vector3d;

    let flight = ctx
        .db
        .game_active_taxi_flight()
        .character_guid()
        .find(character_guid)?;
    if flight.started_micros == 0 {
        return None;
    }
    let points = flight_route(&flight, joined_route(ctx, &leg_paths(ctx, &flight))?).points;
    let elapsed = crate::time::now(ctx)
        .to_micros_since_unix_epoch()
        .saturating_sub(flight.started_micros)
        .max(0);
    let position = route_position(&points, elapsed)?;
    if position.complete {
        return None;
    }
    let duration_ms =
        (route_duration_micros(&points)? / 1_000).clamp(1, i64::from(u32::MAX)) as u32;
    let coords = |point: &GameTaxiPathNode| [point.x, point.y, point.z];
    let [x, y, z] = extrapolated_start(coords(&points[0]), coords(&points[1]));
    let mut nodes = Vec::with_capacity(points.len() + 2);
    nodes.push(Vector3d { x, y, z });
    nodes.extend(points.iter().map(|point| Vector3d {
        x: point.x,
        y: point.y,
        z: point.z,
    }));
    nodes.push(*nodes.last()?);
    Some((
        lyracore_protocol::codec::CreateSpline {
            nodes,
            time_passed_ms: (elapsed / 1_000).clamp(0, i64::from(duration_ms - 1)) as u32,
            duration_ms,
            id: flight.started_micros as u64 as u32,
        },
        [position.x, position.y, position.z, position.orientation],
    ))
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

impl WorldEntity {
    /// The flight presentation is set. A passenger that enters the world mid-flight lacks it, and
    /// can be targeted, until the flight resumes.
    pub(crate) fn is_flying(&self) -> bool {
        self.unit_flags & lyracore_shared::constants::unit_flags::TAXI_FLIGHT != 0
    }

    /// Presents the Character riding `mount_display_id` on a taxi, out of client control.
    pub(crate) fn board_flight(&mut self, mount_display_id: u32) {
        self.mount_display_id = mount_display_id;
        self.unit_flags |= lyracore_shared::constants::unit_flags::TAXI_FLIGHT
            | lyracore_shared::constants::unit_flags::REMOVE_CLIENT_CONTROL;
    }

    pub(crate) fn leave_flight(&mut self) {
        self.mount_display_id = 0;
        self.unit_flags &= !(lyracore_shared::constants::unit_flags::TAXI_FLIGHT
            | lyracore_shared::constants::unit_flags::REMOVE_CLIENT_CONTROL);
        self.movement_flags = 0;
    }
}

pub(crate) fn prepare_world_entry(ctx: &ReducerContext, character: &mut WorldEntity) {
    if is_in_flight(ctx, character.guid) {
        character.leave_flight();
    }
}

/// vmangos `SaveTaxiDestinationsToString` keeps only the current leg, so `ContinueTaxiFlight`
/// flies that leg alone on its source node's mount and lands at its end; later legs go unpaid.
fn keep_only_current_leg(
    ctx: &ReducerContext,
    flight: &mut ActiveTaxiFlight,
    race: u8,
) -> Option<()> {
    let legs = leg_paths(ctx, flight);
    if legs.len() == 1 {
        return Some(());
    }
    delete_onward_legs(ctx, flight.character_guid);
    let path = ctx
        .db
        .game_taxi_path()
        .id()
        .find(*legs.get(flight.current_leg_index as usize)?)?;
    let source = ctx.db.game_taxi_node().id().find(path.source_node_id)?;
    let mount_display_id = faction_mount_display(race, &source);
    if mount_display_id != 0 {
        flight.mount_display_id = mount_display_id;
    }
    flight.path_id = path.id;
    flight.source_node_id = path.source_node_id;
    flight.destination_node_id = path.destination_node_id;
    flight.fare = path.fare;
    flight.current_leg_index = 0;
    Some(())
}

pub(crate) fn resume_flight(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    let Some(mut flight) = ctx
        .db
        .game_active_taxi_flight()
        .character_guid()
        .find(character_guid)
    else {
        return Ok(());
    };
    let Some(character) = ctx.db.game_world_entity().guid().find(character_guid) else {
        return Ok(());
    };
    let Some(route) = keep_only_current_leg(ctx, &mut flight, character.race())
        .and_then(|()| FlightRoute::load(ctx, &flight, &character))
    else {
        clear_flight(ctx, character);
        return Ok(());
    };
    // Spline launch replaces the selected Reference node with the saved position.
    flight.first_destination_node_index = resume_node(&route.route.points, &character) + 1;
    flight.origin_x = character.x;
    flight.origin_y = character.y;
    flight.origin_z = character.z;
    flight.origin_recorded = true;
    flight.current_node_index = 0;
    flight.started_micros = crate::time::now(ctx).to_micros_since_unix_epoch();
    let points = flight_route(&flight, route.route).points;
    let Some(position) = route_position(&points, 0) else {
        clear_flight(ctx, character);
        return Ok(());
    };
    crate::combat::suspend_hostile_references(ctx, character_guid);
    let Some(mut character) = ctx.db.game_world_entity().guid().find(character_guid) else {
        return Ok(());
    };
    let spline = remaining_spline(&flight, &character, &points, position, 0);
    let mount_display_id = flight.mount_display_id;
    ctx.db
        .game_active_taxi_flight()
        .character_guid()
        .update(flight);
    crate::native::taxi::resumed(ctx, &spline)?;
    character.board_flight(mount_display_id);
    character.movement_flags = if points.len() > 1 {
        movement_flags::FORWARD
    } else {
        0
    };
    character.update(ctx);
    crate::native::motion_permit::refresh_character(ctx, character_guid);
    write_passenger_spline(ctx, spline);
    if ctx
        .db
        .game_taxi_flight_schedule()
        .scheduled_id()
        .find(character_guid)
        .is_none()
    {
        ctx.db
            .game_taxi_flight_schedule()
            .insert(TaxiFlightSchedule {
                scheduled_id: character_guid,
                scheduled_at: crate::time::schedule_interval(
                    ctx,
                    TimeDuration::from_micros(TAXI_TICK_MICROS),
                ),
            });
    }
    Ok(())
}

fn write_passenger_spline(ctx: &ReducerContext, row: TaxiPassengerSpline) {
    let splines = ctx.db.game_taxi_passenger_spline();
    if splines.character_guid().find(row.character_guid).is_some() {
        splines.character_guid().update(row);
    } else {
        splines.insert(row);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct LandingPosition {
    map_id: u32,
    x: f32,
    y: f32,
    z: f32,
    orientation: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct FlightEndPlan {
    landing: Option<LandingPosition>,
    instance_id: u64,
    zone_id: Option<u32>,
}

fn project_flight_end(
    player: &mut WorldEntity,
    landing: Option<LandingPosition>,
    zone_id: Option<u32>,
) {
    if let Some(landing) = landing {
        player.map_id = landing.map_id;
        // Landing takes the destination zone below; it explores nothing.
        player
            .place(landing.x, landing.y, landing.z, MoveClock::Unchanged)
            .unexplored();
        player.orientation = landing.orientation;
        if let Some(zone_id) = zone_id {
            player.zone_id = zone_id;
        }
    }
    player.leave_flight();
}

/// Ends an active flight at `plan.landing`, or where the flyer is when there is none, and
/// removes the flight's spline and schedule either way.
fn finish_flight(ctx: &ReducerContext, mut player: WorldEntity, plan: FlightEndPlan) {
    let character_guid = player.guid;
    if ctx
        .db
        .game_active_taxi_flight()
        .character_guid()
        .delete(character_guid)
    {
        project_flight_end(&mut player, plan.landing, plan.zone_id);
        if plan.landing.is_some() && plan.zone_id.is_some() {
            let area = crate::terrain::area_at(ctx, player.map_id, player.x, player.y);
            crate::rest::check_rest_state(ctx, &mut player, area.as_ref());
        }
        player.update(ctx);
        if let Some(landing) = plan.landing {
            crate::motion::drop_pending(ctx, character_guid);
            if let Some(mut character) = crate::helpers::character_by_guid(ctx, character_guid) {
                character.map_id = landing.map_id;
                character.x = landing.x;
                character.y = landing.y;
                character.z = landing.z;
                character.orientation = landing.orientation;
                character.pending_instance_id = plan.instance_id;
                if let Some(zone_id) = plan.zone_id {
                    character.zone_id = zone_id;
                }
                ctx.db.game_character().guid().update(character);
            }
        }
    }
    delete_onward_legs(ctx, character_guid);
    ctx.db
        .game_taxi_passenger_spline()
        .character_guid()
        .delete(character_guid);
    ctx.db
        .game_taxi_flight_schedule()
        .scheduled_id()
        .delete(character_guid);
}

fn clear_flight(ctx: &ReducerContext, player: WorldEntity) {
    let character_guid = player.guid;
    let mounted = player.mount_display_id != 0;
    finish_flight(
        ctx,
        player,
        FlightEndPlan {
            landing: None,
            instance_id: 0,
            zone_id: None,
        },
    );
    crate::native::taxi::aborted(ctx, character_guid, mounted);
}

fn exact_landing(destination: &GameTaxiNode, orientation: f32) -> LandingPosition {
    LandingPosition {
        map_id: destination.map_id,
        x: destination.x,
        y: destination.y,
        z: destination.z,
        orientation,
    }
}

fn land_flight(
    ctx: &ReducerContext,
    player: WorldEntity,
    destination: &GameTaxiNode,
    orientation: f32,
) {
    let character_guid = player.guid;
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
    crate::native::taxi::landed(ctx, character_guid);
}

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
) -> Result<(), crate::Refusal> {
    if is_in_flight(ctx, character_guid) {
        Err("PLAYER_IN_TAXI_FLIGHT".into())
    } else {
        Ok(())
    }
}

/// vmangos `SendTaxiMenu` sends every known node; the client plans a route through them.
fn available_client_nodes(
    ctx: &ReducerContext,
    character_guid: u64,
    source: &GameTaxiNode,
) -> Vec<u32> {
    let catalogue = ctx.db.game_taxi_node();
    let mut available: Vec<u32> = ctx
        .db
        .game_character_taxi_node()
        .by_character()
        .filter(&character_guid)
        .filter_map(|known| catalogue.id().find(known.node_id))
        .map(|node| node.client_node_id)
        .chain(std::iter::once(source.client_node_id))
        .collect();
    available.sort_unstable();
    available.dedup();
    available
}

const TAXI_REPLY_STALE_AFTER_MICROS: i64 = 60_000_000;

fn stale_reply_ids(replies: impl IntoIterator<Item = (u64, i64)>, now_micros: i64) -> Vec<u64> {
    let cutoff = now_micros.saturating_sub(TAXI_REPLY_STALE_AFTER_MICROS);
    replies
        .into_iter()
        .filter_map(|(request_id, created_micros)| (created_micros <= cutoff).then_some(request_id))
        .collect()
}

fn write_reply(ctx: &ReducerContext, reply: TaxiServiceReply) {
    let replies = ctx.db.game_taxi_service_reply();
    let character_guid = reply.character_guid;
    let stale: Vec<_> = replies
        .by_character()
        .filter(&character_guid)
        .map(|row| (row.request_id, row.created_micros))
        .collect();
    for request_id in stale_reply_ids(stale, crate::time::now(ctx).to_micros_since_unix_epoch()) {
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

#[spacetimedb::reducer]
pub fn gw_taxi_node_status(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    npc_guid: u64,
    request_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let character_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    write_reply(
        ctx,
        taxi_node_status(ctx, character_guid, npc_guid, request_id),
    );
    Ok(())
}

/// The node `nearest_taxi_node` finds for the creature; no interaction check.
fn status_node(
    ctx: &ReducerContext,
    character_guid: u64,
    npc_guid: u64,
) -> Result<GameTaxiNode, TaxiGateDenied> {
    let player = crate::helpers::acting_entity_by_guid(ctx, character_guid)
        .ok_or(TaxiGateDenied::PlayerMissing)?;
    let creature = ctx
        .db
        .game_world_entity()
        .guid()
        .find(npc_guid)
        .filter(|npc| {
            !npc.is_player()
                && npc.type_mask & lyracore_shared::constants::type_mask::CREATURE
                    == lyracore_shared::constants::type_mask::CREATURE
                && crate::helpers::in_same_partition(npc, player.map_id, player.instance_id)
        })
        .ok_or(TaxiGateDenied::FlightMasterMissing)?;
    nearest_taxi_node(&creature, player.race(), ctx.db.game_taxi_node().iter())
        .ok_or(TaxiGateDenied::NoSourceNode)
}

pub(crate) fn taxi_node_status(
    ctx: &ReducerContext,
    character_guid: u64,
    npc_guid: u64,
    request_id: u64,
) -> TaxiServiceReply {
    match status_node(ctx, character_guid, npc_guid) {
        Ok(source) => TaxiServiceReply {
            character_guid,
            request_id,
            operation: lyracore_shared::constants::taxi_protocol::REPLY_STATUS,
            npc_guid,
            accepted: true,
            known: is_known(ctx, character_guid, source.id),
            source_client_node_id: source.client_node_id,
            available_client_node_ids: Vec::new(),
            refusal: String::new(),
            created_micros: crate::time::now(ctx).to_micros_since_unix_epoch(),
            result_code: lyracore_shared::constants::taxi_protocol::ACTIVATE_OK,
        },
        Err(denied) => refused_reply(
            character_guid,
            request_id,
            lyracore_shared::constants::taxi_protocol::REPLY_STATUS,
            npc_guid,
            denied,
            crate::time::now(ctx).to_micros_since_unix_epoch(),
        ),
    }
}

#[spacetimedb::reducer]
pub fn gw_open_taxi(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    npc_guid: u64,
    request_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let character_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    write_reply(ctx, open_taxi(ctx, character_guid, npc_guid, request_id));
    Ok(())
}

/// vmangos `HandleTaxiQueryAvailableNodes`: teaches and announces an unknown node, else opens the menu.
pub(crate) fn open_taxi(
    ctx: &ReducerContext,
    character_guid: u64,
    npc_guid: u64,
    request_id: u64,
) -> TaxiServiceReply {
    match resolve_flight_master(ctx, character_guid, npc_guid) {
        Ok((_player, source)) if discover(ctx, character_guid, source.id) => accepted_open_reply(
            ctx,
            character_guid,
            request_id,
            npc_guid,
            lyracore_shared::constants::taxi_protocol::REPLY_NEW_PATH,
            &source,
            Vec::new(),
        ),
        Ok((_player, source)) => menu_reply(ctx, character_guid, request_id, npc_guid, &source),
        Err(denied) => refused_open_reply(ctx, character_guid, request_id, npc_guid, denied),
    }
}

/// vmangos `SendTaxiMenu`: the menu of a chosen gossip option, which never teaches the node.
pub(crate) fn taxi_menu(
    ctx: &ReducerContext,
    character_guid: u64,
    npc_guid: u64,
    request_id: u64,
) -> TaxiServiceReply {
    match resolve_flight_master(ctx, character_guid, npc_guid) {
        Ok((_player, source)) => menu_reply(ctx, character_guid, request_id, npc_guid, &source),
        Err(denied) => refused_open_reply(ctx, character_guid, request_id, npc_guid, denied),
    }
}

fn menu_reply(
    ctx: &ReducerContext,
    character_guid: u64,
    request_id: u64,
    npc_guid: u64,
    source: &GameTaxiNode,
) -> TaxiServiceReply {
    accepted_open_reply(
        ctx,
        character_guid,
        request_id,
        npc_guid,
        lyracore_shared::constants::taxi_protocol::REPLY_OPEN,
        source,
        available_client_nodes(ctx, character_guid, source),
    )
}

fn accepted_open_reply(
    ctx: &ReducerContext,
    character_guid: u64,
    request_id: u64,
    npc_guid: u64,
    operation: u8,
    source: &GameTaxiNode,
    available_client_node_ids: Vec<u32>,
) -> TaxiServiceReply {
    TaxiServiceReply {
        character_guid,
        request_id,
        operation,
        npc_guid,
        accepted: true,
        known: true,
        source_client_node_id: source.client_node_id,
        available_client_node_ids,
        refusal: String::new(),
        created_micros: crate::time::now(ctx).to_micros_since_unix_epoch(),
        result_code: lyracore_shared::constants::taxi_protocol::ACTIVATE_OK,
    }
}

fn refused_open_reply(
    ctx: &ReducerContext,
    character_guid: u64,
    request_id: u64,
    npc_guid: u64,
    denied: TaxiGateDenied,
) -> TaxiServiceReply {
    refused_reply(
        character_guid,
        request_id,
        lyracore_shared::constants::taxi_protocol::REPLY_OPEN,
        npc_guid,
        denied,
        crate::time::now(ctx).to_micros_since_unix_epoch(),
    )
}

#[spacetimedb::reducer]
pub fn gw_activate_taxi(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    npc_guid: u64,
    source_client_node_id: u32,
    destination_client_node_id: u32,
    request_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let character_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    if let Some(reply) = activate_route(
        ctx,
        character_guid,
        npc_guid,
        &[source_client_node_id, destination_client_node_id],
        request_id,
    ) {
        write_reply(ctx, reply);
    }
    Ok(())
}

/// A taxi request along `client_nodes`, as vmangos `Player::ActivateTaxiPathTo` takes it.
pub(crate) fn activate_route(
    ctx: &ReducerContext,
    character_guid: u64,
    npc_guid: u64,
    client_nodes: &[u32],
    request_id: u64,
) -> Option<TaxiServiceReply> {
    let created_micros = crate::time::now(ctx).to_micros_since_unix_epoch();
    let attempt = (|| -> Result<(), TaxiGateDenied> {
        // vmangos refuses a flying Character before anything else, and without a reply.
        if has_active_flight(ctx, character_guid) {
            return Err(TaxiGateDenied::Flying);
        }
        let (mut player, _) = interacting_flight_master(ctx, character_guid, npc_guid)?;
        if client_nodes.len() < 2 {
            return Err(TaxiGateDenied::NoPath);
        }
        let mut player_state = ActivationPlayerState::from_world(ctx, &player);
        player_state.validate_can_act()?;
        let catalogue = ctx.db.game_taxi_node();
        let nodes = client_nodes
            .iter()
            .map(|&client_node_id| {
                catalogue
                    .client_node_id()
                    .find(client_node_id)
                    .filter(|node| is_known(ctx, character_guid, node.id))
                    .ok_or(TaxiGateDenied::UnknownNode)
            })
            .collect::<Result<Vec<_>, _>>()?;
        player_state.validate_can_mount()?;
        source_node_in_reach(&player, &nodes[0])?;
        let legs = route_legs(ctx, &nodes)?;
        let (source, destination) = (&nodes[0], &nodes[nodes.len() - 1]);
        let mount_display_id = faction_mount_display(player.race(), source);
        let leg_fares: Vec<u32> = legs.iter().map(|leg| leg.fare).collect();
        player_state.activate(&leg_fares, mount_display_id)?;

        for (leg_index, leg) in (1u32..).zip(&legs[1..]) {
            ctx.db.game_taxi_flight_leg().insert(TaxiFlightLeg {
                id: 0,
                character_guid,
                leg_index,
                path_id: leg.id,
            });
        }
        let flight = ActiveTaxiFlight {
            character_guid,
            path_id: legs[0].id,
            source_node_id: source.id,
            destination_node_id: destination.id,
            mount_display_id,
            fare: legs[0].fare,
            current_node_index: 0,
            // Queue the paid reply before arming motion so movement cannot overtake it.
            started_micros: 0,
            origin_x: player.x,
            origin_y: player.y,
            origin_z: player.z,
            origin_recorded: true,
            first_destination_node_index: 1,
            current_leg_index: 0,
        };
        ctx.db.game_active_taxi_flight().insert(flight);
        crate::native::motion_permit::refresh_character(ctx, character_guid);
        player.money = player_state.money;
        player.update(ctx);
        Ok(())
    })();

    if attempt.is_err_and(TaxiGateDenied::is_silent) {
        return None;
    }
    Some(activation_reply(
        character_guid,
        request_id,
        npc_guid,
        client_nodes.first().copied().unwrap_or_default(),
        created_micros,
        attempt,
    ))
}

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
    let Some(player) = ctx.db.game_world_entity().guid().find(character_guid) else {
        return;
    };
    let Some(route) = FlightRoute::load(ctx, &flight, &player) else {
        clear_flight(ctx, player);
        return;
    };
    let points = flight_route(&flight, route.route).points;
    crate::combat::disengage(ctx, character_guid);
    let Some(mut player) = ctx.db.game_world_entity().guid().find(character_guid) else {
        return;
    };
    flight.started_micros = crate::time::now(ctx).to_micros_since_unix_epoch();
    player.board_flight(flight.mount_display_id);
    player.movement_flags = movement_flags::FORWARD;
    let Some(start) = route_position(&points, 0) else {
        clear_flight(ctx, player);
        return;
    };
    let start = RoutePosition {
        x: player.x,
        y: player.y,
        z: player.z,
        ..start
    };
    let spline = remaining_spline(&flight, &player, &points, start, 0);
    ctx.db
        .game_active_taxi_flight()
        .character_guid()
        .update(flight);
    player.update(ctx);
    crate::native::taxi::spline_changed(ctx, &spline);
    write_passenger_spline(ctx, spline);
    ctx.db
        .game_taxi_flight_schedule()
        .insert(TaxiFlightSchedule {
            scheduled_id: character_guid,
            scheduled_at: crate::time::schedule_interval(
                ctx,
                TimeDuration::from_micros(TAXI_TICK_MICROS),
            ),
        });
}

#[spacetimedb::reducer]
pub fn gw_arm_taxi_flight(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let character_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    arm_taxi_flight(ctx, character_guid);
    Ok(())
}

#[spacetimedb::reducer]
pub fn advance_taxi_flight(ctx: &ReducerContext, schedule: TaxiFlightSchedule) {
    if !crate::time::scheduler_authorized(ctx) {
        return;
    }
    taxi_pass(ctx, schedule);
}

fn taxi_pass(ctx: &ReducerContext, schedule: TaxiFlightSchedule) {
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
        // An offline flyer's flight waits for `resume_flight`, which re-arms this schedule.
        ctx.db
            .game_taxi_flight_schedule()
            .scheduled_id()
            .delete(character_guid);
        return;
    };
    let Some(route) = FlightRoute::load(ctx, &flight, &player) else {
        clear_flight(ctx, player);
        return;
    };
    let flown = flight_route(&flight, route.route);
    let elapsed = crate::time::now(ctx)
        .to_micros_since_unix_epoch()
        .saturating_sub(flight.started_micros);
    let Some(position) = route_position(&flown.points, elapsed) else {
        clear_flight(ctx, player);
        return;
    };
    let leg = flown.leg_at(position.point_index);
    pay_entered_legs(ctx, &route.legs, &mut flight, &mut player, leg);
    let points = flown.points;
    if position.complete {
        let orientation = if points.len() == 1 {
            player.orientation
        } else {
            position.orientation
        };
        land_flight(ctx, player, &route.destination, orientation);
        return;
    }

    let previous_cell = (player.grid_x, player.grid_y);
    player
        .place(position.x, position.y, position.z, MoveClock::Unchanged)
        .unexplored();
    let area = crate::terrain::area_at(ctx, player.map_id, player.x, player.y);
    let zone_id = area.as_ref().map(crate::terrain::zone_of);
    crate::world::apply_zone_transition(ctx, &mut player, zone_id);
    if zone_id.is_some() {
        crate::rest::check_rest_state(ctx, &mut player, area.as_ref());
    }
    let (grid_x, grid_y) = (player.grid_x, player.grid_y);
    player.orientation = position.orientation;
    player.movement_flags = movement_flags::FORWARD;
    let crossed_point = flight.current_node_index != position.point_index as u32;
    flight.current_node_index = position.point_index as u32;
    let spline = (crossed_point || previous_cell != (grid_x, grid_y))
        .then(|| remaining_spline(&flight, &player, &points, position, elapsed));
    if let Some(spline) = spline {
        write_passenger_spline(ctx, spline);
    }
    player.update(ctx);
    ctx.db
        .game_active_taxi_flight()
        .character_guid()
        .update(flight);
}

#[spacetimedb::reducer]
pub fn gw_ack_taxi_reply(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    request_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let character_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let replies = ctx.db.game_taxi_service_reply();
    if replies
        .request_id()
        .find(request_id)
        .as_ref()
        .is_some_and(|reply| reply.character_guid == character_guid)
    {
        replies.request_id().delete(request_id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flying_player() -> WorldEntity {
        let mut player = crate::helpers::tests::entity(7, 0, 0, 32, 32);
        (player.x, player.y, player.z, player.orientation) = (-9500.0, 25.0, 60.0, 0.75);
        player.zone_id = 12;
        player.mount_display_id = 1147;
        player.unit_flags = 0x0010_0040;
        player.movement_flags = movement_flags::FORWARD;
        player
    }

    fn flight_master_and_player(npc_x: f32) -> (WorldEntity, WorldEntity) {
        let player = crate::helpers::tests::entity(1, 0, 0, 32, 32);
        let mut npc = crate::helpers::tests::entity(2, 0, 0, 32, 32);
        npc.x = npc_x;
        npc.npc_flags = 0x0000_0008;
        (player, npc)
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
            own_mount_horde: true,
            own_mount_alliance: true,
            in_taxi_network: true,
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
    fn a_flight_master_is_in_range_up_to_exactly_ten_yards() {
        let (player, npc) = flight_master_and_player(10.0);
        assert_eq!(flight_master_gate(&player, &npc), Ok(()));
        let (player, npc) = flight_master_and_player(10.01);
        assert_eq!(
            flight_master_gate(&player, &npc),
            Err(TaxiGateDenied::OutOfRange)
        );
    }

    #[test]
    fn a_not_selectable_flight_master_is_not_interactable() {
        let (player, mut npc) = flight_master_and_player(1.0);
        npc.unit_flags = 0x0200_0000;
        assert_eq!(
            flight_master_gate(&player, &npc),
            Err(TaxiGateDenied::NotInteractable)
        );
    }

    #[test]
    fn a_flight_master_in_combat_is_not_interactable() {
        let (player, mut npc) = flight_master_and_player(1.0);
        npc.unit_flags = lyracore_shared::constants::unit_flags::IN_COMBAT;
        assert_eq!(
            flight_master_gate(&player, &npc),
            Err(TaxiGateDenied::FlightMasterInCombat)
        );
    }

    #[test]
    fn the_packet_source_node_is_in_reach_up_to_the_vmangos_squared_distance() {
        let (player, _) = flight_master_and_player(0.0);
        let at = |x, map_id| GameTaxiNode {
            x,
            y: 1.0,
            ..node(1, 1, map_id)
        };
        assert_eq!(source_node_in_reach(&player, &at(30.0, 0)), Ok(()));
        assert_eq!(
            source_node_in_reach(&player, &at(32.0, 0)),
            Err(TaxiGateDenied::SourceTooFar)
        );
        assert_eq!(
            source_node_in_reach(&player, &at(1.0, 1)),
            Err(TaxiGateDenied::SourceTooFar)
        );
        assert_eq!(
            source_node_in_reach(&player, &node(1, 1, 0)),
            Err(TaxiGateDenied::NodeWithoutPosition)
        );
    }

    #[test]
    fn combat_refuses_before_a_mount_does_and_a_mount_before_a_cast() {
        use lyracore_shared::constants::unit_flags;
        let idle = ActivationPlayerState {
            has_active_flight: false,
            money: 100,
            mount_display_id: 0,
            form_blocks_mounting: false,
            casting: false,
            unit_flags: 0,
        };
        let mounted_in_combat = ActivationPlayerState {
            mount_display_id: 1147,
            unit_flags: unit_flags::IN_COMBAT,
            ..idle
        };
        assert_eq!(
            mounted_in_combat.validate_can_act(),
            Err(TaxiGateDenied::PlayerBusy)
        );
        let busy_mounted_caster = ActivationPlayerState {
            mount_display_id: 1147,
            form_blocks_mounting: true,
            casting: true,
            ..idle
        };
        assert_eq!(
            busy_mounted_caster.validate_can_mount(),
            Err(TaxiGateDenied::PlayerAlreadyMounted)
        );
        assert_eq!(
            ActivationPlayerState {
                mount_display_id: 0,
                ..busy_mounted_caster
            }
            .validate_can_mount(),
            Err(TaxiGateDenied::PlayerShapeShifted)
        );
        assert_eq!(idle.validate_can_mount(), Ok(()));
        assert_eq!(
            ActivationPlayerState {
                casting: true,
                ..idle
            }
            .validate_can_mount(),
            Err(TaxiGateDenied::PlayerBusy)
        );
    }

    #[test]
    fn the_taxi_node_is_the_nearest_eligible_on_the_map_however_far() {
        let (_, npc) = flight_master_and_player(0.0);
        let at = |id, x, map_id| GameTaxiNode {
            x,
            ..node(id, id, map_id)
        };
        let nearest = |nodes: Vec<GameTaxiNode>| {
            nearest_taxi_node(&npc, 1, nodes.into_iter()).map(|node| node.id)
        };
        assert_eq!(nearest(vec![at(1, 5000.0, 0)]), Some(1));
        assert_eq!(nearest(vec![at(1, 2.0, 1)]), None);
        assert_eq!(nearest(vec![at(1, 6.0, 0), at(2, 3.0, 0)]), Some(2));
        assert_eq!(nearest(vec![at(9, -3.0, 0), at(4, 3.0, 0)]), Some(4));
    }

    #[test]
    fn the_taxi_node_skips_a_node_outside_the_network_or_without_the_team_mount() {
        let (_, npc) = flight_master_and_player(0.0);
        let pick = |race, tweak: fn(&mut GameTaxiNode)| {
            let mut near = GameTaxiNode {
                x: 1.0,
                ..node(1, 1, 0)
            };
            tweak(&mut near);
            let far = GameTaxiNode {
                x: 50.0,
                ..node(2, 2, 0)
            };
            nearest_taxi_node(&npc, race, [near, far].into_iter()).map(|node| node.id)
        };
        assert_eq!(pick(1, |_| {}), Some(1));
        assert_eq!(pick(1, |node| node.in_taxi_network = false), Some(2));
        assert_eq!(pick(2, |node| node.own_mount_horde = false), Some(2));
        assert_eq!(pick(1, |node| node.own_mount_horde = false), Some(1));
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
    fn every_activation_refusal_builds_a_committable_stable_reply() {
        use lyracore_shared::constants::taxi_protocol as wire;
        let silent = [
            TaxiGateDenied::PlayerMissing,
            TaxiGateDenied::PlayerDead,
            TaxiGateDenied::PlayerLostControl,
            TaxiGateDenied::FlightMasterMissing,
            TaxiGateDenied::FlightMasterDead,
            TaxiGateDenied::FlightMasterInCombat,
            TaxiGateDenied::NotFlightMaster,
            TaxiGateDenied::DifferentPartition,
            TaxiGateDenied::OutOfRange,
            TaxiGateDenied::Hostile,
            TaxiGateDenied::NotInteractable,
            TaxiGateDenied::Flying,
            TaxiGateDenied::NoClientControl,
            TaxiGateDenied::UnknownNode,
            TaxiGateDenied::NoPath,
        ];
        let answered = [
            (TaxiGateDenied::NoSourceNode, wire::ACTIVATE_NO_SUCH_PATH),
            (TaxiGateDenied::PlayerBusy, wire::ACTIVATE_PLAYER_BUSY),
            (
                TaxiGateDenied::PlayerAlreadyMounted,
                wire::ACTIVATE_PLAYER_ALREADY_MOUNTED,
            ),
            (
                TaxiGateDenied::PlayerShapeShifted,
                wire::ACTIVATE_PLAYER_SHAPE_SHIFTED,
            ),
            (
                TaxiGateDenied::NodeWithoutPosition,
                wire::ACTIVATE_UNSPECIFIED_SERVER_ERROR,
            ),
            (TaxiGateDenied::SourceTooFar, wire::ACTIVATE_TOO_FAR_AWAY),
            (
                TaxiGateDenied::UnsupportedRoute,
                wire::ACTIVATE_NO_SUCH_PATH,
            ),
            (
                TaxiGateDenied::MissingMount,
                wire::ACTIVATE_UNSPECIFIED_SERVER_ERROR,
            ),
            (
                TaxiGateDenied::NotEnoughMoney,
                wire::ACTIVATE_NOT_ENOUGH_MONEY,
            ),
        ];
        let replies = silent
            .into_iter()
            .map(|denied| (denied, true, wire::ACTIVATE_UNSPECIFIED_SERVER_ERROR))
            .chain(
                answered
                    .into_iter()
                    .map(|(denied, status)| (denied, false, status)),
            );
        for (denied, is_silent, expected) in replies {
            let reply = activation_reply(7, 11, 90, 255, 1234, Err(denied));
            assert!(!reply.accepted, "{denied:?}");
            assert_eq!(reply.result_code, expected, "{denied:?}");
            assert!(!reply.refusal.is_empty(), "{denied:?}");
            assert_eq!(reply.operation, wire::REPLY_ACTIVATE);
            assert_eq!((reply.character_guid, reply.request_id), (7, 11));
            assert_eq!(denied.is_silent(), is_silent, "{denied:?}");
        }
        let accepted = activation_reply(7, 11, 90, 255, 1234, Ok(()));
        assert!(accepted.accepted);
        assert_eq!(accepted.result_code, wire::ACTIVATE_OK);
        assert!(accepted.refusal.is_empty());
    }

    #[test]
    fn fare_debit_is_exact_and_never_wraps() {
        assert_eq!(debit_first_leg(100, &[25]), Ok(75));
        assert_eq!(debit_first_leg(25, &[25]), Ok(0));
        assert_eq!(
            debit_first_leg(24, &[25]),
            Err(TaxiGateDenied::NotEnoughMoney)
        );
    }

    #[test]
    fn a_route_needs_money_for_every_leg_and_pays_only_the_first() {
        assert_eq!(debit_first_leg(55, &[25, 30]), Ok(30));
        assert_eq!(
            debit_first_leg(54, &[25, 30]),
            Err(TaxiGateDenied::NotEnoughMoney)
        );
        assert_eq!(
            debit_first_leg(u32::MAX, &[u32::MAX, 1]),
            Ok(0),
            "a saturated route fare still pays the first leg"
        );
    }

    #[test]
    fn legs_join_at_the_vmangos_default_joints() {
        let leg = |path_id: u32, len: u32| -> Vec<GameTaxiPathNode> {
            (0..len).map(|index| point(0, path_id, index, 0)).collect()
        };
        let route = RoutePoints::join(vec![leg(1, 4), leg(2, 3), leg(3, 3)]);
        let joined: Vec<_> = route
            .points
            .iter()
            .map(|point| (point.path_id, point.node_index))
            .collect();
        assert_eq!(joined, [(1, 0), (1, 1), (1, 2), (2, 1), (3, 1), (3, 2)]);
        assert_eq!(route.leg_ends, [2, 3]);
        let legs: Vec<_> = (0..route.points.len())
            .map(|index| route.leg_at(index))
            .collect();
        assert_eq!(legs, [0, 0, 1, 2, 2, 2]);
    }

    #[test]
    fn a_single_leg_keeps_every_point() {
        let route = RoutePoints::join(vec![vec![point(1, 9, 0, 0), point(2, 9, 1, 0)]]);
        assert_eq!(route.points.len(), 2);
        assert!(route.leg_ends.is_empty());
    }

    #[test]
    fn faction_mount_selection_is_deterministic_and_preserves_missing_zero() {
        let mut source = node(1, 1, 0);
        source.mount_display_horde = 295;
        source.mount_display_alliance = 1147;
        assert_eq!(faction_mount_display(2, &source), 295);
        assert_eq!(faction_mount_display(1, &source), 1147);
        source.mount_display_horde = 0;
        assert_eq!(faction_mount_display(6, &source), 0);
    }

    #[test]
    fn a_node_counts_for_the_team_that_has_its_own_mount() {
        let mut node = node(1, 1, 0);
        node.own_mount_horde = false;
        assert!(!has_own_mount(2, &node));
        assert!(has_own_mount(1, &node));
        node.own_mount_alliance = false;
        assert!(!has_own_mount(1, &node));
    }

    #[test]
    fn activation_state_machine_mutates_success_and_charges_at_most_once() {
        let mut state = ActivationPlayerState {
            has_active_flight: false,
            money: 100,
            mount_display_id: 0,
            form_blocks_mounting: false,
            casting: false,
            unit_flags: 0,
        };

        assert_eq!(state.activate(&[25], 1147), Ok(()));
        assert!(state.has_active_flight);
        assert_eq!(state.money, 75);
        assert_eq!(state.mount_display_id, 1147);
        assert_ne!(
            state.unit_flags & lyracore_shared::constants::unit_flags::TAXI_FLIGHT,
            0
        );

        let after_first = state;
        assert_eq!(state.activate(&[25], 1147), Err(TaxiGateDenied::Flying));
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
            form_blocks_mounting: false,
            casting: false,
            unit_flags: 0,
        };
        let before = state;

        assert_eq!(
            state.activate(&[25], 1147),
            Err(TaxiGateDenied::NotEnoughMoney)
        );
        assert_eq!(state, before);
    }

    #[test]
    fn authoritative_route_clock_covers_start_progress_and_exact_landing() {
        let points = vec![point(1, 99, 0, 0), point(2, 99, 1, 0), point(3, 99, 2, 0)];
        let first_leg = route_timestamps_micros(&points).unwrap()[1];
        let total = route_duration_micros(&points).unwrap();
        assert_eq!(total, 63_000);

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
    }

    #[test]
    fn curved_route_uses_float_segment_lengths_and_accumulated_milliseconds() {
        let coordinates = [
            (5.0, -16.0, -1.0),
            (9.0, -14.0, -28.0),
            (27.0, 8.0, -6.0),
            (2.0, -19.0, -8.0),
        ];
        let points: Vec<_> = coordinates
            .into_iter()
            .enumerate()
            .map(|(index, (x, y, z))| {
                let mut point = point(index as u32 + 1, 99, index as u32, 0);
                (point.x, point.y, point.z) = (x, y, z);
                point
            })
            .collect();
        assert_eq!(
            route_timestamps_micros(&points),
            Some(vec![0, 874_000, 2_016_000, 3_173_000])
        );
    }

    #[test]
    fn a_resume_uses_a_strict_segment_bracket() {
        let points: Vec<_> = (0..3)
            .map(|index| {
                let mut node = point(index + 1, 99, index, 0);
                node.x = index as f32 * 10.0;
                node
            })
            .collect();
        let mut character = crate::helpers::tests::entity(1, 0, 0, 0, 0);
        character.x = 15.0;
        assert_eq!(resume_node(&points, &character), 2);
        character.x = 10.0;
        assert_eq!(resume_node(&points, &character), 0);
    }

    #[test]
    fn a_recorded_departure_replaces_the_first_route_point() {
        let mut flight = ActiveTaxiFlight {
            character_guid: 1,
            path_id: 99,
            source_node_id: 2,
            destination_node_id: 4,
            mount_display_id: 6852,
            fare: 110,
            current_node_index: 0,
            started_micros: 1,
            origin_x: 10.0,
            origin_y: 20.0,
            origin_z: 30.0,
            origin_recorded: false,
            first_destination_node_index: 1,
            current_leg_index: 0,
        };

        let points = || RoutePoints::join(vec![vec![point(1, 99, 0, 0), point(2, 99, 1, 0)]]);
        assert_eq!(flight_route(&flight, points()).points[0].x, 0.0);
        flight.origin_recorded = true;
        let route = flight_route(&flight, points()).points;
        assert_eq!((route[0].x, route[0].y, route[0].z), (10.0, 20.0, 30.0));
        assert_eq!((route[1].x, route[1].y, route[1].z), (1.0, 0.0, 0.0));
    }

    #[test]
    fn a_resumed_route_keeps_its_leg_ends_on_the_flown_points() {
        let mut flight = ActiveTaxiFlight {
            character_guid: 1,
            path_id: 1,
            source_node_id: 2,
            destination_node_id: 4,
            mount_display_id: 6852,
            fare: 110,
            current_node_index: 0,
            started_micros: 1,
            origin_x: 0.0,
            origin_y: 0.0,
            origin_z: 0.0,
            origin_recorded: true,
            first_destination_node_index: 1,
            current_leg_index: 0,
        };
        let leg = |path_id: u32, len: u32| -> Vec<GameTaxiPathNode> {
            (0..len).map(|index| point(0, path_id, index, 0)).collect()
        };
        let route = || RoutePoints::join(vec![leg(1, 4), leg(2, 4)]);
        assert_eq!(flight_route(&flight, route()).leg_ends, [2]);
        flight.first_destination_node_index = 2;
        let flown = flight_route(&flight, route());
        assert_eq!(flown.points.len(), 5);
        assert_eq!(flown.leg_ends, [1]);
        assert_eq!(flown.points[1].node_index, 2);
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
    fn a_landing_places_the_player_at_the_destination_and_clears_the_flight_presentation() {
        let mut player = flying_player();
        let landing = LandingPosition {
            map_id: 0,
            x: -9443.5,
            y: 58.25,
            z: 56.75,
            orientation: 1.25,
        };
        project_flight_end(&mut player, Some(landing), Some(40));
        assert_eq!(
            (
                player.map_id,
                player.x,
                player.y,
                player.z,
                player.orientation
            ),
            (0, -9443.5, 58.25, 56.75, 1.25)
        );
        assert_eq!(player.zone_id, 40);
        assert_eq!(
            (
                player.mount_display_id,
                player.unit_flags,
                player.movement_flags
            ),
            (0, 0x0000_0040, 0)
        );
    }

    #[test]
    fn a_cancelled_flight_keeps_the_last_position_and_zone_and_clears_the_presentation() {
        let mut player = flying_player();
        project_flight_end(&mut player, None, Some(999));
        assert_eq!(
            (
                player.map_id,
                player.x,
                player.y,
                player.z,
                player.orientation
            ),
            (0, -9500.0, 25.0, 60.0, 0.75)
        );
        assert_eq!(player.zone_id, 12);
        assert_eq!(
            (
                player.mount_display_id,
                player.unit_flags,
                player.movement_flags
            ),
            (0, 0x0000_0040, 0)
        );
    }
}
