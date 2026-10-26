use lyracore_protocol::messages::vanilla::TransportInfo;
use lyracore_shared::movement_flags::ON_TRANSPORT;
use lyracore_shared::spatial;
use spacetimedb::{reducer, table, ReducerContext, ScheduleAt, Table, TimeDuration};

use crate::config::game_taxi_path_node;
use crate::world::{game_world_entity, WorldEntity};

pub(crate) mod timetable;

use timetable::{PathPoint, Pose, Timetable};

const HIGHGUID_MO_TRANSPORT: u64 = 0x1FC0;
const CARRY_TICK_MICROS: i64 = 500_000;
// CMaNGOS refuses movement with larger offsets, which can hold continent coordinates.
const MAX_OFFSET_XY: f32 = 50.0;
const MAX_OFFSET_Z: f32 = 100.0;
const BOARDING_REACH_YD: f32 = 30.0;

/// One imported MO_TRANSPORT route. `period_ms` is the client's cycle from the world database.
#[table(
    accessor = game_transport,
    public,
    index(accessor = by_map, btree(columns = [map_id]))
)]
pub struct Transport {
    #[primary_key]
    pub entry: u32,
    pub name: String,
    pub display_id: u32,
    pub size: f32,
    pub flags: u32,
    pub taxi_path_id: u32,
    pub move_speed: u32,
    pub accel_rate: u32,
    pub period_ms: u32,
    pub map_id: u32,
}

#[table(
    accessor = game_transport_passenger,
    index(accessor = by_transport, btree(columns = [transport_entry]))
)]
pub struct TransportPassenger {
    #[primary_key]
    pub character_guid: u64,
    pub transport_entry: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub orientation: f32,
}

#[table(
    accessor = game_transport_carry_schedule,
    scheduled(carry_transport_passengers)
)]
pub struct TransportCarrySchedule {
    #[primary_key]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}

crate::character_owned!(delete, fn sweep_delete_game_transport_passenger(ctx, character_guid) {
    ctx.db.game_transport_passenger().character_guid().delete(character_guid);
});
crate::character_owned!(not_transported, fn sweep_transfer_game_transport_passenger());

pub(crate) fn guid(entry: u32) -> u64 {
    (HIGHGUID_MO_TRANSPORT << 48) | u64::from(entry)
}

fn entry_of(guid: u64) -> Option<u32> {
    (guid >> 48 == HIGHGUID_MO_TRANSPORT)
        .then(|| u32::try_from(guid & 0xFFFF_FFFF_FFFF).ok())
        .flatten()
}

/// Transports run only in the open world, where every Character on their map sees them.
pub(crate) fn visible_on(
    ctx: &ReducerContext,
    map_id: u32,
    instance_id: u64,
) -> impl Iterator<Item = u64> + '_ {
    ctx.db
        .game_transport()
        .by_map()
        .filter(map_id)
        .filter(move |_| instance_id == 0)
        .map(|transport| guid(transport.entry))
}

pub(crate) fn now_ms(ctx: &ReducerContext) -> u64 {
    u64::try_from(ctx.timestamp.to_micros_since_unix_epoch() / 1000).unwrap_or(0)
}

pub(crate) fn find(ctx: &ReducerContext, guid: u64) -> Option<Transport> {
    ctx.db.game_transport().entry().find(entry_of(guid)?)
}

pub(crate) fn pose(ctx: &ReducerContext, transport: &Transport, now_ms: u64) -> Option<Pose> {
    Some(timetable(ctx, transport)?.pose(path_progress_ms(transport, now_ms)))
}

/// The phase within the current cycle; docs/architecture.md explains why it is not an uptime.
pub(crate) fn path_progress_ms(transport: &Transport, now_ms: u64) -> u32 {
    (now_ms % u64::from(transport.period_ms.max(1))) as u32
}

fn timetable(ctx: &ReducerContext, transport: &Transport) -> Option<Timetable> {
    let mut nodes: Vec<_> = ctx
        .db
        .game_taxi_path_node()
        .by_path_id()
        .filter(transport.taxi_path_id)
        .collect();
    nodes.sort_by_key(|node| node.node_index);
    let points: Vec<_> = nodes
        .iter()
        .map(|node| PathPoint {
            map_id: node.map_id,
            position: [node.x, node.y, node.z],
            flags: node.flags,
            delay_ms: u32::try_from(node.delay_ms).unwrap_or(0),
        })
        .collect();
    Timetable::build(
        &points,
        transport.move_speed,
        transport.accel_rate,
        transport.period_ms,
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Boarding {
    Refused,
    /// The mover is a Passenger, or was one until this movement.
    Carried,
    Ashore,
}

/// Boards, moves or removes a Passenger from the Transport its movement names.
pub(crate) fn passenger_moved(
    ctx: &ReducerContext,
    mover: &WorldEntity,
    previous_flags: u32,
    transport: Option<&TransportInfo>,
) -> Boarding {
    if !mover.is_player() || (previous_flags | mover.movement_flags) & ON_TRANSPORT == 0 {
        return Boarding::Ashore;
    }
    let passengers = ctx.db.game_transport_passenger();
    let aboard = passengers.character_guid().find(mover.guid);
    let boarded = match claim(mover, transport) {
        Claim::Refused => return Boarding::Refused,
        Claim::Ground => None,
        Claim::Transport(guid, offset) => find(ctx, guid)
            .filter(|transport| transport.map_id == mover.map_id)
            .filter(|transport| {
                aboard
                    .as_ref()
                    .is_some_and(|row| row.transport_entry == transport.entry)
                    || within_reach(ctx, transport, mover, offset)
            })
            .map(|transport| TransportPassenger {
                character_guid: mover.guid,
                transport_entry: transport.entry,
                x: offset[0],
                y: offset[1],
                z: offset[2],
                orientation: offset[3],
            }),
    };
    let Some(passenger) = boarded else {
        return if passengers.character_guid().delete(mover.guid) {
            Boarding::Carried
        } else {
            Boarding::Ashore
        };
    };
    let schedules = ctx.db.game_transport_carry_schedule();
    if schedules
        .scheduled_id()
        .find(u64::from(passenger.transport_entry))
        .is_none()
    {
        schedules.insert(TransportCarrySchedule {
            scheduled_id: u64::from(passenger.transport_entry),
            scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(CARRY_TICK_MICROS)),
        });
    }
    if aboard.is_some() {
        passengers.character_guid().update(passenger);
    } else {
        passengers.insert(passenger);
    }
    Boarding::Carried
}

enum Claim {
    Ground,
    Refused,
    Transport(u64, [f32; 4]),
}

fn claim(mover: &WorldEntity, transport: Option<&TransportInfo>) -> Claim {
    if mover.movement_flags & ON_TRANSPORT == 0 || mover.instance_id != 0 {
        return Claim::Ground;
    }
    let Some(tail) = transport else {
        return Claim::Ground;
    };
    let offset = [
        tail.position.x,
        tail.position.y,
        tail.position.z,
        tail.orientation,
    ];
    let plausible = offset.iter().all(|value| value.is_finite())
        && offset[0].abs() <= MAX_OFFSET_XY
        && offset[1].abs() <= MAX_OFFSET_XY
        && offset[2].abs() <= MAX_OFFSET_Z;
    if plausible {
        Claim::Transport(tail.guid.guid(), offset)
    } else {
        Claim::Refused
    }
}

fn within_reach(
    ctx: &ReducerContext,
    transport: &Transport,
    mover: &WorldEntity,
    offset: [f32; 4],
) -> bool {
    pose(ctx, transport, now_ms(ctx)).is_some_and(|pose| {
        let [x, y, _, _] = pose.carry(offset);
        (x - mover.x).hypot(y - mover.y) <= BOARDING_REACH_YD
    })
}

pub(crate) fn leave(ctx: &ReducerContext, character_guid: u64) {
    ctx.db
        .game_transport_passenger()
        .character_guid()
        .delete(character_guid);
}

pub(crate) fn passenger(ctx: &ReducerContext, character_guid: u64) -> Option<TransportPassenger> {
    ctx.db
        .game_transport_passenger()
        .character_guid()
        .find(character_guid)
}

#[reducer]
pub fn carry_transport_passengers(ctx: &ReducerContext, schedule: TransportCarrySchedule) {
    if ctx.sender() != ctx.database_identity() {
        return;
    }
    let passengers: Vec<_> = u32::try_from(schedule.scheduled_id)
        .map(|entry| {
            ctx.db
                .game_transport_passenger()
                .by_transport()
                .filter(entry)
                .collect()
        })
        .unwrap_or_default();
    let pose = u32::try_from(schedule.scheduled_id)
        .ok()
        .and_then(|entry| ctx.db.game_transport().entry().find(entry))
        .and_then(|transport| pose(ctx, &transport, now_ms(ctx)));
    let Some(pose) = pose.filter(|_| !passengers.is_empty()) else {
        ctx.db
            .game_transport_carry_schedule()
            .scheduled_id()
            .delete(schedule.scheduled_id);
        for passenger in passengers {
            leave(ctx, passenger.character_guid);
        }
        return;
    };
    for passenger in passengers {
        let entity = ctx
            .db
            .game_world_entity()
            .guid()
            .find(passenger.character_guid)
            .filter(|entity| entity.map_id == pose.map_id && entity.instance_id == 0);
        match entity {
            Some(entity) => carry(ctx, entity, pose.carry(offset(&passenger))),
            None => leave(ctx, passenger.character_guid),
        }
    }
}

fn offset(passenger: &TransportPassenger) -> [f32; 4] {
    [passenger.x, passenger.y, passenger.z, passenger.orientation]
}

fn carry(ctx: &ReducerContext, mut entity: WorldEntity, [x, y, z, orientation]: [f32; 4]) {
    let previous_cell = entity.cell;
    let (grid_x, grid_y) = spatial::grid_cell(x, y);
    entity.x = x;
    entity.y = y;
    entity.z = z;
    entity.orientation = orientation;
    entity.grid_x = grid_x;
    entity.grid_y = grid_y;
    entity.cell = spatial::grid_cell_id(grid_x, grid_y);
    if entity.cell != previous_cell && !entity.dead {
        if let Some(area) = crate::terrain::area_at(ctx, entity.map_id, x, y) {
            crate::exploration::check_area_exploration(ctx, &mut entity, &area);
            crate::world::apply_zone_transition(
                ctx,
                &mut entity,
                Some(crate::terrain::zone_of(&area)),
            );
        }
    }
    entity.update(ctx);
}
