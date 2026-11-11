use spacetimedb::{table, ReducerContext};

use crate::actor::{ActionRefusal, ActionRefusalKind};
use crate::game_area_trigger;

#[table(accessor = game_areatrigger_teleport)]
pub struct AreatriggerTeleport {
    #[primary_key]
    pub trigger_id: u32,
    pub target_map: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub o: f32,
    pub name: String,
}

#[derive(Clone, Copy)]
pub(crate) struct AreaTriggerRoute {
    pub source_map: u32,
    pub source_x: f32,
    pub source_y: f32,
    pub source_z: f32,
    pub target_map: u32,
    target_x: f32,
    target_y: f32,
    target_z: f32,
    target_o: f32,
    volume: AreaTriggerVolume,
}

impl AreaTriggerRoute {
    fn valid(self) -> bool {
        let coordinates = [
            self.source_x,
            self.source_y,
            self.source_z,
            self.target_x,
            self.target_y,
            self.target_z,
            self.target_o,
        ];
        coordinates.into_iter().all(f32::is_finite) && self.volume.valid()
    }

    pub(crate) fn contains(self, x: f32, y: f32, z: f32) -> bool {
        self.valid()
            && self
                .volume
                .contains([self.source_x, self.source_y, self.source_z], [x, y, z])
    }
}

#[derive(Clone, Copy)]
pub(crate) struct AreaTriggerVolume {
    radius: f32,
    length: f32,
    width: f32,
    height: f32,
    yaw: f32,
}

impl AreaTriggerVolume {
    pub(crate) fn of(trigger: &crate::config::GameAreaTrigger) -> Self {
        Self {
            radius: trigger.radius,
            length: trigger.box_length,
            width: trigger.box_width,
            height: trigger.box_height,
            yaw: trigger.box_yaw,
        }
    }

    fn valid(self) -> bool {
        [self.radius, self.length, self.width, self.height, self.yaw]
            .into_iter()
            .all(f32::is_finite)
            && (self.radius > 0.0
                || (self.radius == 0.0
                    && self.length > 0.0
                    && self.width > 0.0
                    && self.height > 0.0))
    }

    pub(crate) fn contains(self, origin: [f32; 3], point: [f32; 3]) -> bool {
        self.contains_with_tolerance(origin, point, 0.0)
    }

    pub(crate) fn contains_client_position(self, origin: [f32; 3], point: [f32; 3]) -> bool {
        // The client can send the trigger before its next movement update.
        self.contains_with_tolerance(origin, point, 5.0)
    }

    fn contains_with_tolerance(self, origin: [f32; 3], point: [f32; 3], tolerance: f64) -> bool {
        if !self.valid() || !origin.into_iter().chain(point).all(f32::is_finite) {
            return false;
        }
        let [dx, dy, dz] =
            std::array::from_fn(|index| f64::from(point[index]) - f64::from(origin[index]));
        if self.radius > 0.0 {
            return dx * dx + dy * dy + dz * dz <= (f64::from(self.radius) + tolerance).powi(2);
        }
        let (sin, cos) = f64::from(self.yaw).sin_cos();
        let local_x = dx * cos + dy * sin;
        let local_y = -dx * sin + dy * cos;
        local_x.abs() <= f64::from(self.length) / 2.0 + tolerance
            && local_y.abs() <= f64::from(self.width) / 2.0 + tolerance
            && dz.abs() <= f64::from(self.height) / 2.0 + tolerance
    }
}

pub(crate) fn area_trigger_route(
    ctx: &ReducerContext,
    trigger_id: u32,
) -> Option<AreaTriggerRoute> {
    let volume = ctx.db.game_area_trigger().id().find(trigger_id)?;
    let teleport = ctx
        .db
        .game_areatrigger_teleport()
        .trigger_id()
        .find(trigger_id)?;
    let route = AreaTriggerRoute {
        source_map: volume.map_id,
        source_x: volume.x,
        source_y: volume.y,
        source_z: volume.z,
        target_map: teleport.target_map,
        target_x: teleport.x,
        target_y: teleport.y,
        target_z: teleport.z,
        target_o: teleport.o,
        volume: AreaTriggerVolume::of(&volume),
    };
    route.valid().then_some(route)
}

pub(crate) fn area_trigger_destination(
    ctx: &ReducerContext,
    trigger: u32,
) -> Option<crate::transfer::Destination> {
    let route = area_trigger_route(ctx, trigger)?;
    Some(crate::transfer::Destination {
        map_id: route.target_map,
        instance_id: 0,
        x: route.target_x,
        y: route.target_y,
        z: route.target_z,
        o: route.target_o,
    })
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn enter_sessionless_areatrigger(
    ctx: &ReducerContext,
    character_guid: u64,
    trigger_id: u32,
    expected_map: u32,
    expected_instance: u64,
    controller_generation: u64,
) -> Result<u64, ActionRefusal> {
    crate::sessionless::transfer_authority_gate(ctx, character_guid)?;
    crate::sessionless::action_gate(ctx, character_guid)?;
    let route = area_trigger_route(ctx, trigger_id).ok_or_else(|| {
        ActionRefusal::new(
            ActionRefusalKind::MissingResource,
            format!("AreaTrigger {trigger_id} has no imported route"),
        )
    })?;
    if route.target_map != expected_map
        || (crate::instance::is_dungeon_map(expected_map) && expected_instance == 0)
        || (!crate::instance::is_dungeon_map(expected_map) && expected_instance != 0)
    {
        return Err(ActionRefusal::new(
            ActionRefusalKind::OtherPartition,
            "AreaTrigger does not enter the expected party partition",
        ));
    }
    let destination = crate::transfer::Destination {
        map_id: expected_map,
        instance_id: expected_instance,
        x: route.target_x,
        y: route.target_y,
        z: route.target_z,
        o: route.target_o,
    };
    if let Some(relocation_id) = crate::sessionless::completed_relocation(
        ctx,
        character_guid,
        trigger_id,
        (expected_map, expected_instance),
        controller_generation,
    ) {
        return Ok(relocation_id);
    }
    let entity = crate::helpers::live_entity(ctx, character_guid).map_err(|_| {
        ActionRefusal::new(ActionRefusalKind::MissingActor, "Character is not in world")
    })?;
    if entity.dead {
        return Err(ActionRefusal::new(
            ActionRefusalKind::DeadActor,
            "dead Characters cannot enter an AreaTrigger",
        ));
    }
    if crate::taxi::is_in_flight(ctx, character_guid)
        || crate::spell::is_action_blocked(ctx, character_guid)
    {
        return Err(ActionRefusal::new(
            ActionRefusalKind::CannotAct,
            "Character cannot enter an AreaTrigger",
        ));
    }
    if route.source_map != entity.map_id {
        return Err(ActionRefusal::new(
            ActionRefusalKind::OtherPartition,
            "AreaTrigger is on another map",
        ));
    }
    if !route.contains(entity.x, entity.y, entity.z) {
        return Err(ActionRefusal::new(
            ActionRefusalKind::OutOfRange,
            "Character is outside the AreaTrigger",
        ));
    }
    if crate::instance::is_dungeon_map(expected_map) {
        let party_member_at_destination = crate::group::has_known_party_member_in_partition(
            ctx,
            character_guid,
            (expected_map, expected_instance),
        )
        .map_err(|_| {
            ActionRefusal::new(
                ActionRefusalKind::OtherPartition,
                "party partition facts are unavailable",
            )
        })?;
        if !party_member_at_destination {
            return Err(ActionRefusal::new(
                ActionRefusalKind::OtherPartition,
                "no party member is certified in the expected partition",
            ));
        }
    }
    crate::world::teleport_gate(ctx, character_guid, expected_map, expected_instance)?;
    let destination_instance = if crate::instance::is_dungeon_map(expected_map) {
        crate::instance::admit_existing_party_instance(
            ctx,
            character_guid,
            expected_map,
            expected_instance,
        )
        .map_err(|detail| ActionRefusal::new(ActionRefusalKind::CannotAct, detail))?
    } else {
        0
    };
    if destination_instance != expected_instance {
        return Err(ActionRefusal::new(
            ActionRefusalKind::OtherPartition,
            "AreaTrigger resolves to a different party instance",
        ));
    }
    crate::sessionless::relocate(
        ctx,
        character_guid,
        trigger_id,
        crate::transfer::Destination {
            instance_id: destination_instance,
            ..destination
        },
        controller_generation,
    )
}

pub(crate) fn request_enter_areatrigger(
    ctx: &ReducerContext,
    character_guid: u64,
    trigger_id: u32,
) -> Result<(), crate::Refusal> {
    let actor = crate::helpers::live_entity(ctx, character_guid)?;
    let trigger = ctx
        .db
        .game_area_trigger()
        .id()
        .find(trigger_id)
        .ok_or("AreaTrigger is unavailable")?;
    if actor.map_id != trigger.map_id
        || !AreaTriggerVolume::of(&trigger).contains_client_position(
            [trigger.x, trigger.y, trigger.z],
            [actor.x, actor.y, actor.z],
        )
    {
        return Err("Character is outside the AreaTrigger".into());
    }
    apply_enter_areatrigger(ctx, character_guid, trigger_id);
    Ok(())
}

// A trigger may grant exploration credit and teleport; both effects are independent.
pub(crate) fn apply_enter_areatrigger(ctx: &ReducerContext, player_guid: u64, trigger_id: u32) {
    let teleport_target = ctx
        .db
        .game_areatrigger_teleport()
        .trigger_id()
        .find(trigger_id)
        .map(|t| (t.target_map, t.x, t.y, t.z, t.o));
    crate::quest::on_areatrigger_entered(ctx, player_guid, trigger_id);
    if let Some((target_map, x, y, z, o)) = teleport_target {
        let instance_id = if crate::instance::is_dungeon_map(target_map) {
            if crate::native::instance_shard::enter(
                ctx,
                player_guid,
                trigger_id,
                target_map,
                [x, y, z, o],
            ) {
                return;
            }
            match crate::instance::resolve_or_create_instance(ctx, player_guid, target_map) {
                Ok(id) => id,
                Err(e) => {
                    spacetimedb::log::warn!(
                        "areatrigger {trigger_id}: dungeon entry refused for {player_guid}: {e}"
                    );
                    return;
                }
            }
        } else {
            0
        };
        if let Err(refusal) =
            crate::world::teleport(ctx, player_guid, target_map, instance_id, [x, y, z, o])
        {
            spacetimedb::log::info!(
                "areatrigger {trigger_id}: teleport refused for {player_guid}: {refusal}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AreaTriggerRoute, AreaTriggerVolume};

    #[test]
    fn areatrigger_sphere_includes_its_three_dimensional_boundary() {
        let route = AreaTriggerRoute {
            source_map: 0,
            source_x: 10.0,
            source_y: 20.0,
            source_z: 30.0,
            target_map: 36,
            target_x: 0.0,
            target_y: 0.0,
            target_z: 0.0,
            target_o: 0.0,
            volume: AreaTriggerVolume {
                radius: 5.0,
                length: 0.0,
                width: 0.0,
                height: 0.0,
                yaw: 0.0,
            },
        };
        assert!(route.contains(13.0, 24.0, 30.0));
        assert!(!route.contains(13.1, 24.0, 30.0));
        assert!(!route.contains(10.0, 20.0, 35.1));
    }

    #[test]
    fn client_areatrigger_sphere_allows_five_yards_of_position_lag() {
        let volume = AreaTriggerVolume {
            radius: 6.0,
            length: 0.0,
            width: 0.0,
            height: 0.0,
            yaw: 0.0,
        };
        let origin = [-14.3628, -393.38, 64.5605];
        let last_position = [-14.5732, -385.475, 62.4561];
        assert!(!volume.contains(origin, last_position));
        assert!(volume.contains_client_position(origin, last_position));
        assert!(volume.contains_client_position([0.0; 3], [11.0, 0.0, 0.0]));
        assert!(!volume.contains_client_position([0.0; 3], [11.1, 0.0, 0.0]));
        assert!(!volume.contains_client_position([0.0; 3], [0.0, 0.0, 11.1]));
        assert!(!volume.contains_client_position([0.0; 3], [f32::NAN, 0.0, 0.0]));
    }

    #[test]
    fn client_areatrigger_box_bounds_position_lag_in_rotated_axes() {
        let volume = AreaTriggerVolume {
            radius: 0.0,
            length: 8.0,
            width: 2.0,
            height: 4.0,
            yaw: std::f32::consts::FRAC_PI_2,
        };
        let origin = [0.0; 3];
        assert!(volume.contains_client_position(origin, [5.9, 8.9, 7.0]));
        assert!(!volume.contains(origin, [5.9, 8.9, 7.0]));
        for outside in [[6.1, 0.0, 0.0], [0.0, 9.1, 0.0], [0.0, 0.0, 7.1]] {
            assert!(!volume.contains_client_position(origin, outside));
        }
    }

    #[test]
    fn finite_trigger_coordinates_cannot_overflow_the_range_gate() {
        let volume = AreaTriggerVolume {
            radius: f32::MAX,
            length: 0.0,
            width: 0.0,
            height: 0.0,
            yaw: 0.0,
        };
        assert!(volume.contains([0.0; 3], [f32::MAX, 0.0, 0.0]));
        assert!(!volume.contains([-f32::MAX, 0.0, 0.0], [f32::MAX, 0.0, 0.0]));
        assert!(!volume.contains([0.0; 3], [f32::NAN, 0.0, 0.0]));
    }

    #[test]
    fn areatrigger_box_applies_yaw_before_its_half_extents() {
        let route = AreaTriggerRoute {
            source_map: 0,
            source_x: 10.0,
            source_y: 20.0,
            source_z: 30.0,
            target_map: 36,
            target_x: 0.0,
            target_y: 0.0,
            target_z: 0.0,
            target_o: 0.0,
            volume: AreaTriggerVolume {
                radius: 0.0,
                length: 8.0,
                width: 2.0,
                height: 4.0,
                yaw: std::f32::consts::FRAC_PI_2,
            },
        };
        assert!(route.contains(10.5, 24.0, 32.0));
        assert!(!route.contains(11.1, 24.0, 30.0));
        assert!(!route.contains(10.0, 20.0, 32.1));
    }

    #[test]
    fn areatrigger_route_refuses_non_finite_coordinates_and_invalid_shapes() {
        let mut route = AreaTriggerRoute {
            source_map: 0,
            source_x: 10.0,
            source_y: 20.0,
            source_z: 30.0,
            target_map: 36,
            target_x: 1.0,
            target_y: 2.0,
            target_z: 3.0,
            target_o: 0.0,
            volume: AreaTriggerVolume {
                radius: 5.0,
                length: 0.0,
                width: 0.0,
                height: 0.0,
                yaw: 0.0,
            },
        };
        assert!(route.valid());
        route.volume.radius = f32::INFINITY;
        assert!(!route.valid());
        route.volume.radius = 0.0;
        assert!(!route.valid());
        route.volume.length = 8.0;
        route.volume.width = 2.0;
        route.volume.height = 4.0;
        assert!(route.valid());
        route.source_x = f32::NAN;
        assert!(!route.valid());
        route.source_x = 10.0;
        route.target_z = f32::INFINITY;
        assert!(!route.valid());
    }
}
