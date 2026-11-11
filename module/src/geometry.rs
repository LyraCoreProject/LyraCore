pub(crate) fn distance_3d(ax: f32, ay: f32, az: f32, bx: f32, by: f32, bz: f32) -> f32 {
    let (dx, dy, dz) = (ax - bx, ay - by, az - bz);
    (dx * dx + dy * dy + dz * dz).sqrt()
}

pub(crate) fn projectile_travel_ms(distance_yd: f32, speed_yps: f32) -> u32 {
    if speed_yps <= 0.0 || distance_yd <= 0.0 {
        return 0;
    }
    ((distance_yd / speed_yps) * 1000.0).round().max(0.0) as u32
}

// Orientation is radians from +X, increasing counterclockwise toward +Y.
pub(crate) fn is_behind(ax: f32, ay: f32, tx: f32, ty: f32, target_orientation: f32) -> bool {
    let (dx, dy) = (ax - tx, ay - ty);
    if dx == 0.0 && dy == 0.0 {
        return false;
    }
    // The dot product keeps the side boundary symmetric under f32 rounding.
    dx * target_orientation.cos() + dy * target_orientation.sin() <= 0.0
}

// Uses a 180-degree facing arc; vanilla uses roughly 120 degrees.
pub(crate) fn is_facing(ax: f32, ay: f32, attacker_orientation: f32, tx: f32, ty: f32) -> bool {
    let (dx, dy) = (tx - ax, ty - ay);
    if dx == 0.0 && dy == 0.0 {
        return true;
    }
    dx * attacker_orientation.cos() + dy * attacker_orientation.sin() > 0.0
}

#[cfg(test)]
mod tests {
    use super::{is_behind, projectile_travel_ms};

    #[test]
    fn is_behind_rear_hemisphere() {
        use std::f32::consts::{FRAC_PI_2, PI};
        assert!(is_behind(-1.0, 0.0, 0.0, 0.0, 0.0));
        assert!(!is_behind(1.0, 0.0, 0.0, 0.0, 0.0));
        assert!(is_behind(0.0, 1.0, 0.0, 0.0, 0.0));
        assert!(is_behind(0.0, -1.0, 0.0, 0.0, 0.0));
        assert!(!is_behind(1.0, 0.5, 0.0, 0.0, 0.0));
        assert!(!is_behind(1.0, -0.5, 0.0, 0.0, 0.0));
        assert!(is_behind(1.0, 0.0, 0.0, 0.0, PI));
        assert!(!is_behind(-1.0, 0.0, 0.0, 0.0, PI));
        assert!(is_behind(1.0, 0.0, 0.0, 0.0, -PI));
        assert!(is_behind(0.0, -1.0, 0.0, 0.0, FRAC_PI_2));
        assert!(!is_behind(0.0, 1.0, 0.0, 0.0, FRAC_PI_2));
        assert!(!is_behind(0.0, 0.0, 0.0, 0.0, 0.0));
    }

    #[test]
    fn projectile_travel_ms_scales_distance_over_speed() {
        assert_eq!(projectile_travel_ms(21.0, 21.0), 1000);
        assert_eq!(projectile_travel_ms(10.5, 21.0), 500);
        assert_eq!(projectile_travel_ms(0.0, 21.0), 0);
        assert_eq!(projectile_travel_ms(30.0, 0.0), 0);
        assert_eq!(projectile_travel_ms(30.0, -1.0), 0);
    }
}

#[cfg(test)]
mod facing_tests {
    use super::is_facing;
    use std::f32::consts::PI;

    #[test]
    fn front_and_behind_at_the_cardinal_orientations() {
        assert!(is_facing(0.0, 0.0, 0.0, 5.0, 0.0));
        assert!(!is_facing(0.0, 0.0, 0.0, -5.0, 0.0));
        assert!(is_facing(0.0, 0.0, PI, -5.0, 0.0));
        assert!(!is_facing(0.0, 0.0, PI, 5.0, 0.0));
        assert!(is_facing(0.0, 0.0, PI / 2.0, 0.0, 5.0));
        assert!(!is_facing(0.0, 0.0, PI / 2.0, 0.0, -5.0));
    }

    #[test]
    fn the_90_degree_seam_is_exclusive_and_diagonals_resolve() {
        assert!(!is_facing(0.0, 0.0, 0.0, 0.0, 5.0));
        assert!(!is_facing(0.0, 0.0, 0.0, 0.0, -5.0));
        assert!(is_facing(0.0, 0.0, 0.0, 0.1, 5.0));
        assert!(!is_facing(0.0, 0.0, 0.0, -0.1, 5.0));
    }

    #[test]
    fn wrapped_and_negative_orientations_behave() {
        assert!(is_facing(0.0, 0.0, 2.0 * PI, 5.0, 0.001));
        assert!(is_facing(0.0, 0.0, -PI / 2.0, 0.0, -5.0));
        assert!(!is_facing(0.0, 0.0, -PI / 2.0, 0.0, 5.0));
    }

    #[test]
    fn coincident_points_always_face() {
        assert!(is_facing(3.0, -4.0, 1.234, 3.0, -4.0));
    }
}
