use super::*;
use crate::creatures::cycle::{
    AggroTarget, Combatant, EngageSink, Engagement, Gait, Home, IdleCreature, IdleSink, Leg,
    LegInFlight, MotionSink, Pull, Pursuit, PursuitSink, Sensor, Waypoint,
};
use lyracore_shared::constants::speeds;

impl MotionSink for Simulation {
    fn legs_in_flight(&self) -> Vec<LegInFlight> {
        self.slots
            .iter()
            .filter_map(|slot| {
                slot.spline
                    .as_ref()
                    .map(|leg| as_leg(leg, slot.entity.is_none()))
            })
            .collect()
    }

    fn path_obstruction(&self, _leg: &LegInFlight, _now_micros: u64) -> Option<Point> {
        None
    }

    fn movement_suppressed(&self, guid: u64) -> bool {
        self.immobilized.contains(&guid)
    }

    fn commit_position(&mut self, guid: u64, at: Point, moved_ms: u32) {
        let entity = self.slot_mut(guid).entity.as_mut().unwrap();
        let stored = entity.clone();
        place(entity, at);
        entity.last_move_ms = moved_ms;
        let advanced = entity.clone();
        self.advances.push((stored, advanced));
    }

    fn halt(&mut self, leg: &LegInFlight, at: Point, spline_id: u32) {
        let now_micros = self.at_micros;
        let slot = self.slot_mut(leg.guid);
        let entity = slot.entity.as_mut().unwrap();
        place(entity, at);
        slot.spline = Some(spline(entity, at, 0, false, spline_id, now_micros));
    }

    fn drop_leg(&mut self, guid: u64) {
        self.slot_mut(guid).spline = None;
    }
}

impl PursuitSink for Simulation {
    fn pursuits(&self, scope: &TickScope) -> Vec<Pursuit> {
        self.attacks
            .iter()
            .filter_map(|attack| {
                let creature = self.entity(attack.attacker_guid)?;
                if creature.is_player() || creature.dead || !scope.covers(creature.instance_id) {
                    return None;
                }
                let victim = self.entity(attack.target_guid)?;
                let leg = self.slots[self.by_guid[&creature.guid]].spline.as_ref();
                Some(Pursuit {
                    guid: creature.guid,
                    at: point(creature),
                    orientation: creature.orientation,
                    victim_at: point(victim),
                    victim_orientation: victim.orientation,
                    victim_movement_flags: victim.movement_flags,
                    routing: false,
                    leg: leg.map(|s| as_leg(s, false)),
                })
            })
            .collect()
    }

    fn authored_ranged_posture(&self, _guid: u64) -> Option<(f32, f32)> {
        None
    }

    fn caster_hold_range(&self, _guid: u64) -> f32 {
        0.0
    }

    fn face(&mut self, guid: u64, at: Point, orientation: f32, spline_id: u32) {
        let now_micros = self.at_micros;
        let slot = self.slot_mut(guid);
        let entity = slot.entity.as_mut().unwrap();
        place(entity, at);
        entity.orientation = orientation;
        let mut leg = spline(entity, at, 0, false, spline_id, now_micros);
        leg.facing = true;
        leg.facing_angle = orientation;
        slot.spline = Some(leg);
    }
}

impl IdleSink for Simulation {
    fn speed_of(&self, _guid: u64, gait: Gait) -> f32 {
        match gait {
            Gait::Walk => speeds::WALK,
            Gait::Run => speeds::RUN,
        }
    }

    fn navigate(&self, guid: u64, to: (f32, f32), max_step: f32) -> (f32, f32) {
        let entity = self.entity(guid).unwrap();
        crate::creatures::chase_step(entity.x, entity.y, to.0, to.1, max_step, 0.0)
    }

    fn commit_leg(&mut self, guid: u64, leg: Leg, now_ms: u32) {
        let now_micros = self.at_micros;
        let slot = self.slot_mut(guid);
        let entity = slot.entity.as_mut().unwrap();
        slot.spline = Some(spline(
            entity,
            Point {
                x: leg.to.0,
                y: leg.to.1,
                z: leg.z_fallback,
            },
            leg.dur_ms.max(1),
            leg.gait == Gait::Run,
            now_ms,
            now_micros,
        ));
        entity.last_move_ms = now_ms;
        if leg.hold_until_landed {
            entity.leg_ends_ms = now_ms + leg.dur_ms;
        }
    }

    fn idle_creatures(&self, _active: &HashSet<u64>) -> Vec<IdleCreature> {
        unreachable!("the prototype does not run idle behavior")
    }
    fn route_of(&self, _guid: u64) -> Vec<Waypoint> {
        unreachable!("the prototype does not run patrols")
    }
    fn home_of(&self, _guid: u64) -> Option<Home> {
        unreachable!("the prototype does not run home movement")
    }
    fn engaged(&self, _guid: u64) -> bool {
        unreachable!("the prototype does not run idle behavior")
    }
    fn roll(&self) -> u32 {
        unreachable!("the prototype has no random choices")
    }
    fn aim_at_waypoint(&mut self, _guid: u64, _waypoint_id: u64) {
        unreachable!("the prototype does not run patrols")
    }
    fn reached_home(&mut self, _guid: u64) {
        unreachable!("the prototype does not run home movement")
    }
}

impl EngageSink for Simulation {
    fn line_of_sight(&self, _looker: u64, _at: Point) -> bool {
        true
    }

    fn players(&self) -> Vec<AggroTarget> {
        unreachable!("the prototype does not run aggro")
    }
    fn sensing_creatures(&self, _active: &HashSet<u64>) -> Vec<Sensor> {
        unreachable!("the prototype does not run aggro")
    }
    fn hostile(&self, _faction_template: u32, _other: u32) -> bool {
        unreachable!("the prototype does not run aggro")
    }
    fn engage(&mut self, _creature: u64, _victim: u64, _pull: Pull) {
        unreachable!("the prototype does not change engagements")
    }
    fn engagements(&self) -> Vec<Engagement> {
        unreachable!("the prototype does not change combat flags")
    }
    fn enter_combat(&mut self, _guid: u64) {
        unreachable!("the prototype does not change combat flags")
    }
    fn flagged_in_combat(&self, _candidates: &[u64]) -> Vec<Combatant> {
        unreachable!("the prototype does not change combat flags")
    }
    fn leave_combat(&mut self, _guid: u64) {
        unreachable!("the prototype does not change combat flags")
    }
}

pub(super) fn spline(
    entity: &WorldEntity,
    dest: Point,
    dur_ms: u32,
    run: bool,
    spline_id: u32,
    start_micros: u64,
) -> CreatureSpline {
    CreatureSpline {
        guid: entity.guid,
        start_micros,
        dur_ms,
        sx: entity.x,
        sy: entity.y,
        sz: entity.z,
        dx: dest.x,
        dy: dest.y,
        dz: dest.z,
        map_id: entity.map_id,
        instance_id: entity.instance_id,
        grid_x: entity.grid_x,
        grid_y: entity.grid_y,
        spline_id,
        run,
        cell: entity.cell,
        facing: false,
        facing_angle: 0.0,
        path: None,
    }
}

fn point(entity: &WorldEntity) -> Point {
    Point {
        x: entity.x,
        y: entity.y,
        z: entity.z,
    }
}

fn as_leg(leg: &CreatureSpline, mover_gone: bool) -> LegInFlight {
    LegInFlight {
        guid: leg.guid,
        start: Point {
            x: leg.sx,
            y: leg.sy,
            z: leg.sz,
        },
        dest: Point {
            x: leg.dx,
            y: leg.dy,
            z: leg.dz,
        },
        started_micros: leg.start_micros,
        dur_ms: leg.dur_ms,
        map_id: leg.map_id,
        instance_id: leg.instance_id,
        mover_gone,
        waypoints: Vec::new(),
    }
}
