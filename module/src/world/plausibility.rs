use lyracore_shared::movement_flags::{
    AIRBORNE, FIXED_Z, FLYING, HOVER, LEVITATING, ON_TRANSPORT, SAFE_FALL, SWIMMING,
};
use lyracore_shared::spell::taxonomy::A_FLAG;
use spacetimedb::{ReducerContext, Table};

use super::{game_movement_violation, MovementViolation, WorldEntity};
use crate::tables::game_creature_spline;

/// Magnitude: the observed speed as a multiple of the mover's run speed.
pub const MOVE_VIOLATION_SPEED: u8 = 1;
/// Magnitude: the yards crossed in one movement.
pub const MOVE_VIOLATION_TELEPORT: u8 = 2;
/// Magnitude: the yards a landing dropped beyond what its reported fall time allows.
pub const MOVE_VIOLATION_FALL: u8 = 3;
/// Magnitude: the gravity-defying MovementInfo flag bits the mover newly claims without an Aura.
pub const MOVE_VIOLATION_GRAVITY_FLAG: u8 = 4;
/// Magnitude: the yards between the mover and the highest surface under it.
pub const MOVE_VIOLATION_FLIGHT: u8 = 5;
/// Magnitude: the yards the mover stands below the terrain with no surface under it.
pub const MOVE_VIOLATION_UNDER_WORLD: u8 = 6;

const SPEED_LEEWAY: f32 = 1.8;
const TELEPORT_MAX_YD: f32 = 60.0;
const MIN_DT_S: f32 = 0.05;

// Yards of descent per yard run: steeper than any slope the client walks down rather than falls.
const WALK_DESCENT_PER_YD: f32 = 1.5;
const FALL_SLACK_YD: f32 = 5.0;

// A surface this far above the feet still holds the mover, as `vmap::floor_z` probes.
const UNDERFOOT_REACH_YD: f32 = 2.0;
// A jump rises about 1.6 yd.
const FLIGHT_MIN_HEIGHT_YD: f32 = 15.0;
const UNDER_WORLD_MIN_DEPTH_YD: f32 = 15.0;
// Water walking keeps the mover on the surface without the swimming flag.
const LIQUID_SURFACE_REACH_YD: f32 = 3.0;

// A server-driven leg's client can report positions this long after the leg ends.
const SERVER_LEG_GRACE_MICROS: u64 = 2_000_000;

// vmangos AuraType SPELL_AURA_FEATHER_FALL and SPELL_AURA_HOVER, kept in an A_FLAG effect's p1.
const AURA_FEATHER_FALL: i32 = 105;
const AURA_HOVER: i32 = 106;

pub(crate) struct MovementDelta {
    pub opcode: u16,
    pub old_x: f32,
    pub old_y: f32,
    pub old_z: f32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub old_move_ms: u32,
    pub move_time_ms: u32,
    pub old_flags: u32,
    pub flags: u32,
    pub fall_time_ms: u32,
}

impl MovementDelta {
    pub(crate) fn moved(&self) -> bool {
        (self.x - self.old_x).powi(2)
            + (self.y - self.old_y).powi(2)
            + (self.z - self.old_z).powi(2)
            > 0.0001
    }

    fn elapsed_ms(&self) -> u32 {
        (self.move_time_ms.wrapping_sub(self.old_move_ms) as i32).max(0) as u32
    }

    fn enters_new_client_second(&self) -> bool {
        self.move_time_ms / 1000 != self.old_move_ms / 1000
    }

    // Only a new entity has a zero clock: a zero client time leaves the stored clock as it was.
    fn starts_the_clock(&self) -> bool {
        self.old_move_ms == 0 && self.move_time_ms != 0
    }
}

/// Records every implausible part of a Character's movement as a `game_movement_violation` row.
/// It never corrects or refuses the movement.
pub(crate) fn score_and_log_movement(
    ctx: &ReducerContext,
    mover: &WorldEntity,
    delta: &MovementDelta,
) {
    if !mover.is_player() || mover.godmode || delta.starts_the_clock() {
        return;
    }
    let guid = mover.guid;
    let mut violations = Vec::new();
    if !aboard(delta.flags, delta.old_flags) {
        violations.extend(pace_violation(ctx, guid, delta));
    }
    violations.extend(gravity_violations(ctx, mover, delta));
    if violations.is_empty() || is_gm_character(ctx, guid) || on_server_leg(ctx, guid) {
        return;
    }
    let created_at = crate::time::now(ctx);
    for (kind, magnitude) in violations {
        ctx.db.game_movement_violation().insert(MovementViolation {
            id: 0,
            guid,
            kind,
            magnitude,
            x: delta.x,
            y: delta.y,
            z: delta.z,
            created_at,
        });
    }
}

fn pace_violation(ctx: &ReducerContext, guid: u64, delta: &MovementDelta) -> Option<(u8, f32)> {
    let dist_2d = ((delta.x - delta.old_x).powi(2) + (delta.y - delta.old_y).powi(2)).sqrt();
    let dt_s = delta.elapsed_ms() as f32 / 1000.0;
    if dist_2d <= TELEPORT_MAX_YD && dt_s < MIN_DT_S {
        return None;
    }
    let max_speed =
        crate::modifiers::effective_move_speed(ctx, guid, lyracore_shared::constants::speeds::RUN);
    speed_violation(dist_2d, dt_s, max_speed)
}

fn speed_violation(dist_2d: f32, dt_s: f32, max_speed: f32) -> Option<(u8, f32)> {
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

fn gravity_violations(
    ctx: &ReducerContext,
    mover: &WorldEntity,
    delta: &MovementDelta,
) -> Vec<(u8, f32)> {
    let claimed = claimed_gravity_flags(
        delta.flags,
        delta.old_flags,
        delta.enters_new_client_second(),
    );
    let mut position_violations = Vec::new();
    if delta.opcode as u32 == lyracore_shared::opcodes::movement::MSG_MOVE_FALL_LAND
        && !aboard(delta.flags, delta.old_flags)
    {
        let max_speed = crate::modifiers::effective_move_speed(
            ctx,
            mover.guid,
            lyracore_shared::constants::speeds::RUN,
        );
        let unfallen_ms = delta.elapsed_ms().saturating_sub(delta.fall_time_ms);
        position_violations.extend(
            fall_violation(
                delta.old_z - delta.z,
                delta.fall_time_ms,
                unfallen_ms,
                max_speed,
            )
            .map(|excess| (MOVE_VIOLATION_FALL, excess)),
        );
    }
    if delta.enters_new_client_second()
        && on_foot(delta.flags, delta.old_flags)
        && height_checks_enabled(ctx, mover.map_id)
    {
        let (map_id, instance_id) = (mover.map_id, mover.instance_id);
        let (x, y, z) = (delta.x, delta.y, delta.z);
        let collision = || {
            if !crate::vmap::has_collision_cell(ctx, map_id, x, y) {
                return CollisionFloor::Unloaded;
            }
            crate::vmap::floor_z(ctx, map_id, instance_id, x, y, z)
                .map_or(CollisionFloor::Open, CollisionFloor::At)
        };
        position_violations.extend(ground_violation(
            z,
            crate::terrain::ground_z(ctx, map_id, x, y),
            collision,
            || crate::terrain::liquid_level_at(ctx, map_id, x, y),
        ));
    }
    if claimed == 0 && position_violations.is_empty() {
        return position_violations;
    }
    // A feather fall or hover Aura excuses any fall or height.
    let granted = aura_granted_flags(ctx, mover.guid);
    let mut violations = if granted == 0 {
        position_violations
    } else {
        Vec::new()
    };
    let ungranted = claimed & !granted;
    if ungranted != 0 {
        violations.push((MOVE_VIOLATION_GRAVITY_FLAG, ungranted as f32));
    }
    violations
}

/// `unfallen_ms` is the part of the movement before the reported fall began.
fn fall_violation(
    drop_yd: f32,
    fall_time_ms: u32,
    unfallen_ms: u32,
    max_speed: f32,
) -> Option<f32> {
    let fall_s = fall_time_ms as f32 / 1000.0;
    let walked_s = unfallen_ms as f32 / 1000.0;
    let allowed_yd = 0.5 * super::GRAVITY_YD_PER_S2 * fall_s * fall_s
        + max_speed * WALK_DESCENT_PER_YD * walked_s
        + FALL_SLACK_YD;
    (drop_yd > allowed_yd).then_some(drop_yd - allowed_yd)
}

// vmangos `HandleFlagTests` flags these on every packet as fly or fixed-height hacks.
fn flight_flags(flags: u32) -> u32 {
    let swim_flying = if flags & SWIMMING != 0 {
        flags & FLYING
    } else {
        0
    };
    (flags & (LEVITATING | FIXED_Z)) | swim_flying
}

/// The gravity-defying flags this movement claims. Like vmangos, a hover or slow-fall flag counts
/// only when the movement before lacked it, so an Aura that just ended is not flagged. A held
/// fly or fixed-height flag counts again only in a new client second.
fn claimed_gravity_flags(flags: u32, old_flags: u32, new_client_second: bool) -> u32 {
    let flight = flight_flags(flags);
    let counted = if new_client_second {
        flight
    } else {
        flight & !flight_flags(old_flags)
    };
    counted | (flags & !old_flags & (SAFE_FALL | HOVER))
}

fn aura_granted_flags(ctx: &ReducerContext, guid: u64) -> u32 {
    crate::spell::auras_on(ctx, guid)
        .filter(|aura| aura.eff_kind == A_FLAG)
        .fold(0, |granted, aura| {
            granted
                | match aura.eff_p1 {
                    AURA_FEATHER_FALL => SAFE_FALL,
                    AURA_HOVER => HOVER,
                    _ => 0,
                }
        })
}

// An elevator or tram GameObject also sets this, and the Module does not carry its riders.
fn aboard(flags: u32, old_flags: u32) -> bool {
    (flags | old_flags) & ON_TRANSPORT != 0
}

/// Neither this movement nor the one before it is airborne, swimming or aboard a Transport.
fn on_foot(flags: u32, old_flags: u32) -> bool {
    (flags | old_flags) & (AIRBORNE | SWIMMING | ON_TRANSPORT) == 0
}

fn height_checks_enabled(ctx: &ReducerContext, map_id: u32) -> bool {
    crate::config::server_config(ctx).movement_height_checks_enabled
        && crate::vmap::vmap_enabled(ctx, map_id)
}

/// What the active collision generation holds under the mover.
enum CollisionFloor {
    /// The generation has no rows for the mover's cell.
    Unloaded,
    /// No collision surface lies under the mover.
    Open,
    /// The highest collision surface at most `UNDERFOOT_REACH_YD` above the feet.
    At(f32),
}

/// `collision` and `liquid_z` are read only when the terrain alone cannot clear the mover. Without
/// collision rows for the cell there is no verdict.
fn ground_violation(
    z: f32,
    terrain_z: Option<f32>,
    collision: impl FnOnce() -> CollisionFloor,
    liquid_z: impl FnOnce() -> Option<f32>,
) -> Option<(u8, f32)> {
    let terrain_underfoot = terrain_z.filter(|terrain| *terrain <= z + UNDERFOOT_REACH_YD);
    if terrain_underfoot.is_some_and(|terrain| z - terrain <= FLIGHT_MIN_HEIGHT_YD) {
        return None;
    }
    let floor_z = match collision() {
        CollisionFloor::Unloaded => return None,
        CollisionFloor::Open => None,
        CollisionFloor::At(floor) => Some(floor),
    };
    let (kind, yards, threshold) = match (terrain_underfoot, floor_z) {
        (None, None) => (
            MOVE_VIOLATION_UNDER_WORLD,
            terrain_z? - z,
            UNDER_WORLD_MIN_DEPTH_YD,
        ),
        (Some(terrain), Some(floor)) => (
            MOVE_VIOLATION_FLIGHT,
            z - terrain.max(floor),
            FLIGHT_MIN_HEIGHT_YD,
        ),
        (Some(surface), None) | (None, Some(surface)) => {
            (MOVE_VIOLATION_FLIGHT, z - surface, FLIGHT_MIN_HEIGHT_YD)
        }
    };
    (yards > threshold && !afloat(z, liquid_z())).then_some((kind, yards))
}

fn afloat(z: f32, liquid_z: Option<f32>) -> bool {
    liquid_z.is_some_and(|level| z <= level + LIQUID_SURFACE_REACH_YD)
}

fn is_gm_character(ctx: &ReducerContext, guid: u64) -> bool {
    crate::helpers::character_by_guid(ctx, guid)
        .is_some_and(|c| crate::auth::gm_level(ctx, c.account_id) != 0)
}

// Charge and other server-driven legs move the Character along a spline the client follows.
fn on_server_leg(ctx: &ReducerContext, guid: u64) -> bool {
    let now_micros = crate::time::now(ctx).to_micros_since_unix_epoch().max(0) as u64;
    ctx.db
        .game_creature_spline()
        .guid()
        .find(guid)
        .is_some_and(|leg| {
            now_micros
                < leg
                    .start_micros
                    .saturating_add(u64::from(leg.dur_ms) * 1000)
                    .saturating_add(SERVER_LEG_GRACE_MICROS)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN: f32 = lyracore_shared::constants::speeds::RUN;

    fn no_probe() -> Option<f32> {
        panic!("the terrain alone settles this movement")
    }

    fn no_collision_probe() -> CollisionFloor {
        panic!("the terrain alone settles this movement")
    }

    fn floor(z: f32) -> impl FnOnce() -> CollisionFloor {
        move || CollisionFloor::At(z)
    }

    fn open() -> CollisionFloor {
        CollisionFloor::Open
    }

    #[test]
    fn speed_violation_flags_speed_and_teleport_but_not_legit_motion() {
        assert_eq!(speed_violation(3.5, 0.5, RUN), None);
        assert_eq!(speed_violation(RUN * 1.5 * 0.5, 0.5, RUN), None);
        let (k, mag) = speed_violation(RUN * 3.0 * 0.5, 0.5, RUN).unwrap();
        assert_eq!(k, MOVE_VIOLATION_SPEED);
        assert!(
            (mag - 3.0).abs() < 1e-3,
            "magnitude is the ×-normal multiple"
        );
        let (k, mag) = speed_violation(100.0, 0.0, RUN).unwrap();
        assert_eq!(k, MOVE_VIOLATION_TELEPORT);
        assert_eq!(mag, 100.0);
        assert_eq!(speed_violation(5.0, 0.01, RUN), None);
        assert!(
            speed_violation(RUN * 2.0 * 0.5, 0.5, RUN / 2.0).is_some(),
            "snared mover flagged sooner"
        );
    }

    #[test]
    fn a_jump_landing_where_it_started_is_a_plausible_fall() {
        assert_eq!(fall_violation(0.0, 700, 0, RUN), None);
        assert_eq!(
            fall_violation(-1.5, 400, 0, RUN),
            None,
            "a jump onto a ledge"
        );
    }

    #[test]
    fn a_long_fall_whose_fall_time_covers_its_drop_is_plausible() {
        // A 3 s fall from rest drops about 86.8 yd.
        assert_eq!(fall_violation(86.0, 3_000, 0, RUN), None);
        assert_eq!(
            fall_violation(26.0, 3_000, 0, RUN),
            None,
            "the last half second of that fall"
        );
    }

    #[test]
    fn running_down_a_slope_before_stepping_off_it_is_a_plausible_fall() {
        let epic_mount = RUN * 2.0;
        // 0.4 s down a 45-degree slope at 14 yd/s, then a 0.1 s fall.
        assert_eq!(fall_violation(5.7, 100, 400, epic_mount), None);
        assert_eq!(fall_violation(3.6, 50, 450, RUN), None, "stairs on foot");
    }

    #[test]
    fn a_slow_fall_never_drops_further_than_gravity_allows() {
        assert_eq!(fall_violation(14.0, 2_000, 0, RUN), None);
    }

    #[test]
    fn a_big_drop_landed_with_no_fall_time_is_a_fall_violation() {
        let excess = fall_violation(40.0, 0, 500, RUN).expect("fall violation");
        assert!(excess > 25.0, "{excess}");
        assert!(fall_violation(40.0, 300, 0, RUN).is_some());
    }

    #[test]
    fn jumping_falling_and_swimming_are_not_on_foot() {
        use lyracore_shared::movement_flags::{FALLING_FAR, FORWARD, JUMPING};
        assert!(on_foot(FORWARD, 0));
        assert!(!on_foot(FORWARD | JUMPING, FORWARD));
        assert!(
            !on_foot(FORWARD, FORWARD | JUMPING),
            "the landing heartbeat"
        );
        assert!(!on_foot(FALLING_FAR, 0));
        assert!(!on_foot(SWIMMING, 0));
        assert!(!on_foot(0, SWIMMING), "climbing out of the water");
    }

    #[test]
    fn walking_and_jumping_over_terrain_need_no_collision_probe() {
        assert_eq!(
            ground_violation(10.1, Some(10.0), no_collision_probe, no_probe),
            None
        );
        assert_eq!(
            ground_violation(11.6, Some(10.0), no_collision_probe, no_probe),
            None,
            "a jump"
        );
        assert_eq!(
            ground_violation(9.0, Some(10.5), no_collision_probe, no_probe),
            None,
            "a dip in the terrain"
        );
    }

    #[test]
    fn standing_on_a_building_or_bridge_above_the_terrain_is_supported() {
        assert_eq!(
            ground_violation(60.0, Some(10.0), floor(59.8), || None),
            None
        );
        assert_eq!(
            ground_violation(61.5, Some(10.0), floor(60.0), || None),
            None,
            "walking down its stairs"
        );
    }

    #[test]
    fn a_cave_floor_under_the_terrain_supports_the_mover() {
        assert_eq!(
            ground_violation(-40.0, Some(0.0), floor(-40.2), || None),
            None
        );
    }

    #[test]
    fn standing_high_above_every_surface_is_flight() {
        let (kind, height) = ground_violation(40.0, Some(10.0), open, || None).unwrap();
        assert_eq!(kind, MOVE_VIOLATION_FLIGHT);
        assert_eq!(height, 30.0);
        let (kind, height) = ground_violation(80.0, Some(10.0), floor(50.0), || None).unwrap();
        assert_eq!(
            (kind, height),
            (MOVE_VIOLATION_FLIGHT, 30.0),
            "above a roof"
        );
        assert_eq!(
            ground_violation(-10.0, Some(0.0), floor(-40.0), || None).map(|v| v.0),
            Some(MOVE_VIOLATION_FLIGHT),
            "inside a cave"
        );
    }

    #[test]
    fn standing_under_the_terrain_with_nothing_beneath_is_under_the_world() {
        let (kind, depth) = ground_violation(-40.0, Some(0.0), open, || None).unwrap();
        assert_eq!((kind, depth), (MOVE_VIOLATION_UNDER_WORLD, 40.0));
        assert_eq!(
            ground_violation(-5.0, Some(0.0), open, || None),
            None,
            "clipping the terrain's edge"
        );
    }

    #[test]
    fn walking_on_water_over_a_deep_lake_is_supported() {
        assert_eq!(ground_violation(30.0, Some(0.0), open, || Some(30.0)), None);
    }

    #[test]
    fn missing_ground_data_gives_no_verdict() {
        assert_eq!(ground_violation(500.0, None, open, || None), None);
    }

    #[test]
    fn ordinary_movement_flags_claim_no_gravity_defiance() {
        use lyracore_shared::movement_flags::{FALLING_FAR, FORWARD, JUMPING};
        for flags in [
            FORWARD,
            FORWARD | JUMPING,
            FALLING_FAR,
            SWIMMING,
            FLYING,
            ON_TRANSPORT,
        ] {
            assert_eq!(claimed_gravity_flags(flags, 0, true), 0, "{flags:#x}");
        }
    }

    #[test]
    fn a_newly_claimed_fly_hover_or_slow_fall_flag_is_a_gravity_violation() {
        assert_eq!(claimed_gravity_flags(SAFE_FALL, 0, false), SAFE_FALL);
        assert_eq!(claimed_gravity_flags(HOVER | 0x1, 0x1, false), HOVER);
        assert_eq!(claimed_gravity_flags(LEVITATING, 0, false), LEVITATING);
        assert_eq!(claimed_gravity_flags(FIXED_Z, 0, false), FIXED_Z);
        assert_eq!(claimed_gravity_flags(SWIMMING | FLYING, 0, false), FLYING);
    }

    #[test]
    fn a_held_slow_fall_or_hover_flag_is_never_flagged_again() {
        assert_eq!(claimed_gravity_flags(SAFE_FALL, SAFE_FALL, true), 0);
        assert_eq!(claimed_gravity_flags(HOVER, HOVER, true), 0);
        assert_eq!(
            claimed_gravity_flags(SAFE_FALL | HOVER, SAFE_FALL, false),
            HOVER
        );
    }

    #[test]
    fn a_held_fly_or_fixed_height_flag_is_flagged_again_each_client_second() {
        assert_eq!(claimed_gravity_flags(LEVITATING, LEVITATING, false), 0);
        assert_eq!(
            claimed_gravity_flags(LEVITATING, LEVITATING, true),
            LEVITATING
        );
        assert_eq!(claimed_gravity_flags(FIXED_Z, FIXED_Z, true), FIXED_Z);
        let swim_flying = SWIMMING | FLYING;
        assert_eq!(
            claimed_gravity_flags(swim_flying, swim_flying, true),
            FLYING
        );
    }

    #[test]
    fn riding_an_elevator_or_tram_is_aboard_and_not_on_foot() {
        use lyracore_shared::movement_flags::FORWARD;
        assert!(aboard(FORWARD | ON_TRANSPORT, FORWARD));
        assert!(aboard(FORWARD, ON_TRANSPORT), "stepping off");
        assert!(!aboard(FORWARD, FORWARD));
        assert!(!on_foot(ON_TRANSPORT, ON_TRANSPORT));
        assert!(!on_foot(FORWARD, ON_TRANSPORT));
    }

    #[test]
    fn a_cell_without_collision_rows_gives_no_height_verdict() {
        let unloaded = || CollisionFloor::Unloaded;
        assert_eq!(ground_violation(40.0, Some(10.0), unloaded, || None), None);
        assert_eq!(ground_violation(-40.0, Some(0.0), unloaded, || None), None);
    }
}
