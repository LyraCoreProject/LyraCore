use lyracore_protocol::{
    codec,
    messages::vanilla::{opcodes::ServerOpcodeMessage, Vector3d},
    world::Packet,
};
use spacetimedb::{ConnectionId, ReducerContext, Table};

use super::game_native_player;
use crate::{
    aoi::{game_aoi_grant, WORLD_ENTITY},
    game_creature_spline, game_world_entity, CreatureSpline,
};

fn packet(row: &CreatureSpline, replay_at: Option<u64>) -> Option<Packet> {
    let mut start = Vector3d {
        x: row.sx,
        y: row.sy,
        z: row.sz,
    };
    if ![start.x, start.y, start.z].into_iter().all(f32::is_finite) {
        return None;
    }
    if row.facing {
        if !row.facing_angle.is_finite() {
            return None;
        }
        return lyracore_protocol::world::encode(ServerOpcodeMessage::SMSG_MONSTER_MOVE(Box::new(
            codec::build_monster_move_facing(row.guid, start, row.facing_angle, row.spline_id),
        )))
        .ok();
    }
    let mut duration_ms = row.dur_ms;
    let mut points = row.path.as_ref().map(|path| {
        path.points
            .iter()
            .map(|point| (point.x, point.y, point.z))
            .collect::<Vec<_>>()
    });
    if let Some(now) = replay_at {
        let duration = u64::from(row.dur_ms) * 1000;
        let elapsed = now.saturating_sub(row.start_micros);
        if duration == 0 || elapsed >= duration {
            return None;
        }
        let fraction = elapsed as f64 / duration as f64;
        if let Some(points) = &mut points {
            let (at, next) = lyracore_shared::movement_path::sample(
                (row.sx, row.sy, row.sz),
                points,
                fraction as f32,
            );
            (start.x, start.y, start.z) = at;
            points.drain(..next);
        } else {
            start.x =
                (f64::from(row.sx) + (f64::from(row.dx) - f64::from(row.sx)) * fraction) as f32;
            start.y =
                (f64::from(row.sy) + (f64::from(row.dy) - f64::from(row.sy)) * fraction) as f32;
            start.z =
                (f64::from(row.sz) + (f64::from(row.dz) - f64::from(row.sz)) * fraction) as f32;
        }
        duration_ms = (duration - elapsed).div_ceil(1000) as u32;
    }
    let destination = if let Some(points) = points {
        if let Some((opcode, body)) = codec::build_ground_path_raw(
            row.guid,
            start,
            &points,
            duration_ms,
            row.spline_id,
            row.run,
        ) {
            return Some(Packet { opcode, body });
        }
        duration_ms = 0;
        start
    } else {
        Vector3d {
            x: row.dx,
            y: row.dy,
            z: row.dz,
        }
    };
    if ![
        start.x,
        start.y,
        start.z,
        destination.x,
        destination.y,
        destination.z,
    ]
    .into_iter()
    .all(f32::is_finite)
    {
        return None;
    }
    lyracore_protocol::world::encode(ServerOpcodeMessage::SMSG_MONSTER_MOVE(Box::new(
        codec::build_monster_move(
            row.guid,
            start,
            destination,
            duration_ms,
            row.spline_id,
            row.run && duration_ms != 0,
        ),
    )))
    .ok()
}

fn current(ctx: &ReducerContext, row: &CreatureSpline) -> bool {
    ctx.db
        .game_world_entity()
        .guid()
        .find(row.guid)
        .is_some_and(|entity| entity.map_id == row.map_id && entity.instance_id == row.instance_id)
}

pub(crate) fn changed(ctx: &ReducerContext, row: &CreatureSpline) {
    if ctx.db.game_native_player().count() == 0
        || !current(ctx, row)
        || ctx
            .db
            .game_aoi_grant()
            .by_subject()
            .filter((WORLD_ENTITY, row.guid))
            .next()
            .is_none()
    {
        return;
    }
    if let Some(packet) = packet(row, None) {
        super::visibility::broadcast(ctx, row.guid, packet.opcode, packet.body, false);
    } else {
        spacetimedb::log::warn!(
            "creature {} has malformed spline {}",
            row.guid,
            row.spline_id
        );
    }
}

pub(super) fn replay(ctx: &ReducerContext, connection: ConnectionId, guid: u64) {
    use crate::game_encounter_equip;
    if let Some(row) = ctx
        .db
        .game_encounter_equip()
        .creature_guid()
        .find(guid)
        .filter(|row| equipment_is_current(ctx, row))
    {
        let (opcode, body) = equipment_packet(&row, false);
        super::send_raw(ctx, connection, opcode, body, false);
    }
    let now = ctx.timestamp.to_micros_since_unix_epoch().max(0) as u64;
    if let Some(packet) = ctx
        .db
        .game_creature_spline()
        .guid()
        .find(guid)
        .filter(|row| current(ctx, row))
        .and_then(|row| packet(&row, Some(now)))
    {
        super::send_raw(ctx, connection, packet.opcode, packet.body, false);
    }
}

fn equipment_is_current(ctx: &ReducerContext, row: &crate::EncounterEquip) -> bool {
    ctx.db
        .game_world_entity()
        .guid()
        .find(row.creature_guid)
        .is_some_and(|entity| !entity.is_player() && entity.instance_id == row.instance_id)
}

fn equipment_packet(row: &crate::EncounterEquip, cleared: bool) -> (u16, Vec<u8>) {
    let (main_hand, off_hand, ranged) = if cleared {
        (0, 0, 0)
    } else {
        (row.main_hand, row.off_hand, row.ranged)
    };
    codec::build_virtual_item_values(row.creature_guid, main_hand, off_hand, ranged)
}

pub(crate) fn equipment_changed(ctx: &ReducerContext, row: &crate::EncounterEquip, cleared: bool) {
    if equipment_is_current(ctx, row) {
        let (opcode, body) = equipment_packet(row, cleared);
        super::visibility::broadcast(ctx, row.creature_guid, opcode, body, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::creatures::tick::{CreaturePath, CreaturePathPoint};

    fn leg() -> CreatureSpline {
        CreatureSpline {
            guid: 1,
            start_micros: 1_000_000,
            dur_ms: 2000,
            sx: 0.0,
            sy: 0.0,
            sz: 0.0,
            dx: 10.0,
            dy: 0.0,
            dz: 0.0,
            map_id: 0,
            instance_id: 0,
            grid_x: 0,
            grid_y: 0,
            spline_id: 7,
            run: true,
            cell: 0,
            facing: false,
            facing_angle: 0.0,
            path: None,
        }
    }

    fn curved_leg() -> CreatureSpline {
        CreatureSpline {
            dur_ms: 7000,
            dx: 3.0,
            dy: 4.0,
            path: Some(CreaturePath {
                points: vec![
                    CreaturePathPoint {
                        x: 3.0,
                        y: 0.0,
                        z: 0.0,
                    },
                    CreaturePathPoint {
                        x: 3.0,
                        y: 4.0,
                        z: 0.0,
                    },
                ],
                navigation: crate::nav::NavigationInputs {
                    imported_revision: None,
                    navigation_enabled: false,
                    collision_enabled: false,
                    coverage_enabled: false,
                    static_generation: None,
                    coverage_generation: None,
                },
            }),
            ..leg()
        }
    }

    fn word(body: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes(body[offset..offset + 4].try_into().unwrap())
    }

    fn point(body: &[u8], offset: usize) -> [f32; 3] {
        [offset, offset + 4, offset + 8].map(|at| f32::from_bits(word(body, at)))
    }

    #[test]
    fn late_viewer_resumes_a_straight_leg_with_its_original_id() {
        let packet = packet(&leg(), Some(1_500_000)).unwrap();
        assert_eq!(packet.opcode, 0xdd);
        assert_eq!(&packet.body[..2], &[1, 1]);
        assert_eq!(point(&packet.body, 2), [2.5, 0.0, 0.0]);
        assert_eq!(word(&packet.body, 14), 7);
        assert_eq!(word(&packet.body, 19), 0x100);
        assert_eq!(word(&packet.body, 23), 1500);
        assert_eq!(word(&packet.body, 27), 1);
        assert_eq!(point(&packet.body, 31), [10.0, 0.0, 0.0]);
    }

    #[test]
    fn live_curved_leg_encodes_the_corner_before_the_destination() {
        let packet = packet(&curved_leg(), None).unwrap();
        assert_eq!(word(&packet.body, 23), 7000);
        assert_eq!(word(&packet.body, 27), 2);
        assert_eq!(point(&packet.body, 31), [3.0, 4.0, 0.0]);
        assert_eq!(word(&packet.body, 43), 16 << 11);
        assert_eq!(packet.body.len(), 47);
    }

    #[test]
    fn late_viewer_continues_after_a_passed_corner() {
        let packet = packet(&curved_leg(), Some(4_500_000)).unwrap();
        assert_eq!(point(&packet.body, 2), [3.0, 0.5, 0.0]);
        assert_eq!(word(&packet.body, 14), 7);
        assert_eq!(word(&packet.body, 23), 3500);
        assert_eq!(word(&packet.body, 27), 1);
        assert_eq!(point(&packet.body, 31), [3.0, 4.0, 0.0]);
        assert_eq!(packet.body.len(), 43);
    }

    #[test]
    fn replay_clamps_before_start_and_rounds_remaining_milliseconds_up() {
        let row = leg();
        let future = packet(&row, Some(500_000)).unwrap();
        assert_eq!(point(&future.body, 2), [0.0, 0.0, 0.0]);
        assert_eq!(word(&future.body, 23), 2000);
        let last = packet(&row, Some(2_999_999)).unwrap();
        assert_eq!(word(&last.body, 23), 1);
        assert!(packet(&row, Some(3_000_000)).is_none());
        assert!(packet(&row, Some(u64::MAX)).is_none());
    }

    #[test]
    fn stop_is_delivered_live_but_never_replayed() {
        let row = CreatureSpline {
            dur_ms: 0,
            dx: 0.0,
            ..leg()
        };
        let stop = packet(&row, None).unwrap();
        assert_eq!(word(&stop.body, 19), 0);
        assert_eq!(word(&stop.body, 23), 0);
        assert_eq!(point(&stop.body, 31), [0.0, 0.0, 0.0]);
        assert!(packet(&row, Some(row.start_micros)).is_none());
    }

    #[test]
    fn stationary_facing_survives_late_entry() {
        let row = CreatureSpline {
            facing: true,
            facing_angle: 1.5,
            dur_ms: 0,
            ..leg()
        };
        let live = packet(&row, None).unwrap();
        let replay = packet(&row, Some(u64::MAX)).unwrap();
        assert_eq!(live.body, replay.body);
        assert_eq!(replay.body[18], 4);
        assert_eq!(f32::from_bits(word(&replay.body, 19)), 1.5);
    }

    #[test]
    fn unrepresentable_curve_stops_at_the_start() {
        let mut row = curved_leg();
        row.path.as_mut().unwrap().points[0].x = 1000.0;
        let stop = packet(&row, None).unwrap();
        assert_eq!(word(&stop.body, 23), 0);
        assert_eq!(point(&stop.body, 31), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn nonfinite_positions_and_facing_never_reach_the_client() {
        assert!(packet(
            &CreatureSpline {
                sx: f32::NAN,
                ..leg()
            },
            None
        )
        .is_none());
        assert!(packet(
            &CreatureSpline {
                dx: f32::INFINITY,
                ..leg()
            },
            None
        )
        .is_none());
        assert!(packet(
            &CreatureSpline {
                facing: true,
                facing_angle: f32::NAN,
                ..leg()
            },
            None
        )
        .is_none());
    }
}
