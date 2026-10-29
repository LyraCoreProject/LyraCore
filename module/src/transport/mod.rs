use lyracore_protocol::messages::vanilla::TransportInfo;
use lyracore_shared::movement_flags::ON_TRANSPORT;
use spacetimedb::{reducer, table, ReducerContext, ScheduleAt, Table, TimeDuration, Timestamp};

use crate::config::game_taxi_path_node;
use crate::world::{game_world_entity, MoveClock, WorldEntity};

pub(crate) mod timetable;

use timetable::{PathPoint, Pose, Timetable};

const HIGHGUID_MO_TRANSPORT: u64 = 0x1FC0;
const CARRY_TICK_MICROS: i64 = 500_000;
// CMaNGOS refuses movement with larger offsets, which can hold continent coordinates.
const MAX_OFFSET_XY: f32 = 50.0;
const MAX_OFFSET_Z: f32 = 100.0;
const BOARDING_REACH_YD: f32 = 30.0;

/// One imported MO_TRANSPORT route; docs/schema.md defines `period_ms` and `map_id`.
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

#[derive(Clone)]
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

/// The map a route that crosses maps is on now, fired when the Transport next changes map.
#[table(
    accessor = game_transport_crossing,
    scheduled(cross_transport_map),
    index(accessor = by_map, btree(columns = [map_id]))
)]
pub struct TransportCrossing {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    #[unique]
    pub transport_entry: u32,
    pub map_id: u32,
}

pub(crate) struct Aboard {
    pub(crate) passenger: TransportPassenger,
    pub(crate) from_map_id: u32,
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
    let crossings = ctx.db.game_transport_crossing();
    let staying = ctx
        .db
        .game_transport()
        .by_map()
        .filter(map_id)
        .filter(move |transport| crossings.transport_entry().find(transport.entry).is_none());
    let arrived = ctx
        .db
        .game_transport_crossing()
        .by_map()
        .filter(map_id)
        .filter_map(|crossing| {
            ctx.db
                .game_transport()
                .entry()
                .find(crossing.transport_entry)
        });
    staying
        .chain(arrived)
        .filter(move |_| instance_id == 0)
        .map(|transport| guid(transport.entry))
}

pub(crate) fn current_map(ctx: &ReducerContext, transport: &Transport) -> u32 {
    ctx.db
        .game_transport_crossing()
        .transport_entry()
        .find(transport.entry)
        .map_or(transport.map_id, |crossing| crossing.map_id)
}

/// Arms the map change of every route that crosses maps and has none armed.
pub(crate) fn arm_crossings(ctx: &ReducerContext) {
    let transports: Vec<_> = ctx
        .db
        .game_transport()
        .iter()
        .filter(|transport| {
            ctx.db
                .game_transport_crossing()
                .transport_entry()
                .find(transport.entry)
                .is_none()
        })
        .collect();
    for transport in transports {
        let Some(timetable) = timetable(ctx, &transport) else {
            continue;
        };
        if let Some(crossing) = next_crossing(ctx, &transport, &timetable) {
            let map_id = crossing.map_id;
            ctx.db.game_transport_crossing().insert(crossing);
            crate::aoi::transport_moved(ctx, guid(transport.entry), map_id, []);
        }
    }
}

fn next_crossing(
    ctx: &ReducerContext,
    transport: &Transport,
    timetable: &Timetable,
) -> Option<TransportCrossing> {
    let progress_ms = path_progress_ms(transport, now_ms(ctx));
    let (wait_ms, _) = timetable.next_map_change(progress_ms)?;
    Some(TransportCrossing {
        scheduled_id: 0,
        scheduled_at: crate::time::schedule_time(
            ctx,
            Timestamp::from_micros_since_unix_epoch(
                crate::time::now(ctx).to_micros_since_unix_epoch() + i64::from(wait_ms) * 1000,
            ),
        ),
        transport_entry: transport.entry,
        map_id: timetable.pose(progress_ms).map_id,
    })
}

/// Moves a Transport that reached another map, its Passengers and its AOI Grants there.
#[reducer]
pub fn cross_transport_map(ctx: &ReducerContext, fired: TransportCrossing) {
    if !crate::time::scheduler_authorized(ctx) {
        return;
    }
    let crossings = ctx.db.game_transport_crossing();
    if crossings
        .transport_entry()
        .find(fired.transport_entry)
        .is_some_and(|armed| armed.scheduled_id != fired.scheduled_id)
    {
        return;
    }
    // Fired rows remain until reducer return; delete before inserting the next unique entry.
    crossings.transport_entry().delete(fired.transport_entry);
    let Some(transport) = ctx.db.game_transport().entry().find(fired.transport_entry) else {
        crate::aoi::remove_subject(ctx, crate::aoi::TRANSPORT, guid(fired.transport_entry));
        return;
    };
    let timetable = timetable(ctx, &transport);
    let pose = timetable
        .as_ref()
        .map(|timetable| timetable.pose(path_progress_ms(&transport, now_ms(ctx))));
    let next = timetable
        .as_ref()
        .and_then(|timetable| next_crossing(ctx, &transport, timetable));
    let map_id = next.as_ref().map_or(transport.map_id, |next| next.map_id);
    if let Some(next) = next {
        crossings.insert(next);
    }
    if map_id == fired.map_id {
        return;
    }
    let passengers: Vec<_> = ctx
        .db
        .game_transport_passenger()
        .by_transport()
        .filter(transport.entry)
        .collect();
    let mut crossing = Vec::new();
    for passenger in passengers {
        let entity = ctx
            .db
            .game_world_entity()
            .guid()
            .find(passenger.character_guid);
        match (entity, pose) {
            (Some(entity), Some(pose))
                if entity.map_id == fired.map_id && entity.instance_id == 0 =>
            {
                crossing.push((passenger, pose));
            }
            // World entry carries a Passenger still loading the previous map on to this one.
            (None, _) if crate::native::is_world_porting(ctx, passenger.character_guid) => {}
            _ => leave(ctx, passenger.character_guid),
        }
    }
    // Destination observers need the Transport before a sessionless Passenger's CREATE names it.
    crate::aoi::transport_moved(
        ctx,
        guid(transport.entry),
        map_id,
        crossing
            .iter()
            .map(|(passenger, _)| passenger.character_guid),
    );
    for (passenger, pose) in crossing {
        carry_across(ctx, passenger, fired.map_id, pose);
    }
}

fn carry_across(ctx: &ReducerContext, passenger: TransportPassenger, from_map_id: u32, pose: Pose) {
    let world = pose.carry(offset(&passenger));
    crate::world::teleport_aboard(
        ctx,
        pose.map_id,
        world,
        Aboard {
            passenger,
            from_map_id,
        },
    );
}

pub(crate) fn now_ms(ctx: &ReducerContext) -> u64 {
    u64::try_from(crate::time::now(ctx).to_micros_since_unix_epoch() / 1000).unwrap_or(0)
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
    if !mover.is_player() {
        return Boarding::Ashore;
    }
    let passengers = ctx.db.game_transport_passenger();
    let aboard = passengers.character_guid().find(mover.guid);
    if aboard.is_none() && (previous_flags | mover.movement_flags) & ON_TRANSPORT == 0 {
        return Boarding::Ashore;
    }
    let boarded = match claim(mover, transport) {
        Claim::Refused => return Boarding::Refused,
        Claim::Ground => None,
        Claim::Transport(guid, offset) => find(ctx, guid)
            .filter(|transport| current_map(ctx, transport) == mover.map_id)
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
            scheduled_at: crate::time::schedule_interval(
                ctx,
                TimeDuration::from_micros(CARRY_TICK_MICROS),
            ),
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

/// Carries a Passenger whose world port ended on a map its Transport has since left.
pub(crate) fn rejoin(ctx: &ReducerContext, character_guid: u64) {
    let Some(passenger) = passenger(ctx, character_guid) else {
        return;
    };
    let entity = ctx.db.game_world_entity().guid().find(character_guid);
    let transport = ctx
        .db
        .game_transport()
        .entry()
        .find(passenger.transport_entry);
    let target = transport.as_ref().and_then(|transport| {
        Some((
            current_map(ctx, transport),
            pose(ctx, transport, now_ms(ctx))?,
        ))
    });
    // During a pending crossing the client already holds a self CREATE without the Transport.
    match (entity, target) {
        (Some(entity), Some((map_id, pose)))
            if entity.instance_id == 0 && entity.map_id != map_id && pose.map_id == map_id =>
        {
            carry_across(ctx, passenger, entity.map_id, pose);
        }
        _ => leave(ctx, character_guid),
    }
}

/// Restores the attachment a map crossing's teleport ended.
pub(crate) fn reboard(ctx: &ReducerContext, passenger: TransportPassenger) {
    leave(ctx, passenger.character_guid);
    ctx.db.game_transport_passenger().insert(passenger);
}

pub(crate) fn passenger(ctx: &ReducerContext, character_guid: u64) -> Option<TransportPassenger> {
    ctx.db
        .game_transport_passenger()
        .character_guid()
        .find(character_guid)
}

#[reducer]
pub fn carry_transport_passengers(ctx: &ReducerContext, schedule: TransportCarrySchedule) {
    if !crate::time::scheduler_authorized(ctx) {
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
    let transport = u32::try_from(schedule.scheduled_id)
        .ok()
        .and_then(|entry| ctx.db.game_transport().entry().find(entry));
    let pose = transport.as_ref().and_then(|transport| {
        Some((
            current_map(ctx, transport),
            pose(ctx, transport, now_ms(ctx))?,
        ))
    });
    let Some((map_id, pose)) = pose.filter(|_| !passengers.is_empty()) else {
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
            .find(passenger.character_guid);
        match entity {
            Some(entity) if entity.map_id == map_id && entity.instance_id == 0 => {
                // A pose on the next map waits for its crossing to carry the Passenger there.
                if pose.map_id == map_id {
                    carry(ctx, entity, pose.carry(offset(&passenger)));
                }
            }
            None if crate::native::is_world_porting(ctx, passenger.character_guid) => {}
            _ => leave(ctx, passenger.character_guid),
        }
    }
}

fn offset(passenger: &TransportPassenger) -> [f32; 4] {
    [passenger.x, passenger.y, passenger.z, passenger.orientation]
}

fn carry(ctx: &ReducerContext, mut entity: WorldEntity, [x, y, z, orientation]: [f32; 4]) {
    entity.orientation = orientation;
    entity
        .place(x, y, z, MoveClock::Unchanged)
        .explore(ctx, &mut entity);
    entity.update(ctx);
}
