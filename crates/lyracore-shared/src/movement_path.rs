pub type Point = (f32, f32, f32);

pub const MAX_POINTS: usize = 64;
// Vanilla offsets use signed quarter-yard XYZ fields.
pub const MAX_DISTANCE: f32 = 112.0;

pub fn distance(a: Point, b: Point) -> f32 {
    ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2) + (b.2 - a.2).powi(2)).sqrt()
}

pub fn length(start: Point, points: &[Point]) -> f32 {
    std::iter::once(start)
        .chain(points.iter().copied())
        .zip(points.iter().copied())
        .map(|(a, b)| distance(a, b))
        .sum()
}

pub fn sample(start: Point, points: &[Point], fraction: f32) -> (Point, usize) {
    let mut remaining = length(start, points) * fraction.clamp(0.0, 1.0);
    let mut from = start;
    for (index, &to) in points.iter().enumerate() {
        let segment = distance(from, to);
        if remaining < segment && segment > 0.0 {
            let t = remaining / segment;
            return (
                (
                    from.0 + (to.0 - from.0) * t,
                    from.1 + (to.1 - from.1) * t,
                    from.2 + (to.2 - from.2) * t,
                ),
                index,
            );
        }
        remaining -= segment;
        from = to;
    }
    (from, points.len())
}

pub fn progress_2d(start: Point, points: &[Point], position: (f32, f32)) -> Option<f32> {
    let mut from = start;
    let mut travelled = 0.0;
    let mut closest: Option<(f32, f32)> = None;
    for &to in points {
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        let squared = dx * dx + dy * dy;
        if squared > 0.0 {
            let t = (((position.0 - from.0) * dx + (position.1 - from.1) * dy) / squared)
                .clamp(0.0, 1.0);
            let error = (position.0 - from.0 - dx * t).hypot(position.1 - from.1 - dy * t);
            if error <= 0.35 && closest.is_none_or(|old| error < old.0) {
                closest = Some((error, travelled + squared.sqrt() * t));
            }
        }
        travelled += squared.sqrt();
        from = to;
    }
    closest.map(|(_, progress)| progress)
}

// Intermediate spline points are relative to the final destination.
pub fn packed_offset(destination: Point, point: Point) -> Option<u32> {
    let values = [
        (destination.0 - point.0) * 4.0,
        (destination.1 - point.1) * 4.0,
        (destination.2 - point.2) * 4.0,
    ];
    let limits = [(-1024.0, 1023.0), (-1024.0, 1023.0), (-512.0, 511.0)];
    if values
        .iter()
        .zip(limits)
        .any(|(v, (min, max))| !v.is_finite() || v.round() < min || v.round() > max)
    {
        return None;
    }
    let x = values[0].round() as i32 as u32 & 0x7ff;
    let y = values[1].round() as i32 as u32 & 0x7ff;
    let z = values[2].round() as i32 as u32 & 0x3ff;
    Some(x | (y << 11) | (z << 22))
}

fn unpacked_point(destination: Point, packed: u32) -> Point {
    let x = ((packed << 21) as i32 >> 21) as f32 * 0.25;
    let y = ((packed << 10) as i32 >> 21) as f32 * 0.25;
    let z = (packed as i32 >> 22) as f32 * 0.25;
    (destination.0 - x, destination.1 - y, destination.2 - z)
}

// Normalize before collision checks so authoritative movement matches the encoded path.
pub fn wire_points(points: &[Point]) -> Option<Vec<Point>> {
    let &destination = points.last()?;
    if points.len() > MAX_POINTS
        || points
            .iter()
            .any(|p| !p.0.is_finite() || !p.1.is_finite() || !p.2.is_finite())
    {
        return None;
    }
    let mut result = Vec::with_capacity(points.len());
    for &point in &points[..points.len() - 1] {
        let point = unpacked_point(destination, packed_offset(destination, point)?);
        // The vanilla client can stall on an intermediate point too close to the destination.
        if distance(point, destination).powi(2) >= 0.5 && result.last() != Some(&point) {
            result.push(point);
        }
    }
    result.push(destination);
    Some(result)
}

/// Catmull-Rom position weights of the four controls around a segment at `t` in `[0, 1]`.
pub fn catmull_rom_weights(t: f32) -> [f32; 4] {
    let t2 = t * t;
    let t3 = t2 * t;
    [
        -0.5 * t3 + t2 - 0.5 * t,
        1.5 * t3 - 2.5 * t2 + 1.0,
        -1.5 * t3 + 2.0 * t2 + 0.5 * t,
        0.5 * t3 - 0.5 * t2,
    ]
}

/// Catmull-Rom tangent weights: the derivative of `catmull_rom_weights` in `t`.
pub fn catmull_rom_slopes(t: f32) -> [f32; 4] {
    let t2 = t * t;
    [
        -1.5 * t2 + 2.0 * t - 0.5,
        4.5 * t2 - 5.0 * t,
        -4.5 * t2 + 4.0 * t + 0.5,
        1.5 * t2 - t,
    ]
}

pub fn catmull_rom(controls: &[[f32; 3]; 4], weights: [f32; 4]) -> [f32; 3] {
    std::array::from_fn(|axis| {
        controls
            .iter()
            .zip(weights)
            .map(|(point, weight)| point[axis] * weight)
            .sum()
    })
}

/// The control before `first` on an open path: `first` mirrored away from `second`.
pub fn extrapolated_start(first: [f32; 3], second: [f32; 3]) -> [f32; 3] {
    [
        2.0 * first[0] - second[0],
        2.0 * first[1] - second[1],
        2.0 * first[2] - second[2],
    ]
}

/// One straight 2D step of at most `max_step` from `(cx, cy)` toward `(tx, ty)`, stopping
/// `stop_dist` short of it.
pub fn chase_step(cx: f32, cy: f32, tx: f32, ty: f32, max_step: f32, stop_dist: f32) -> (f32, f32) {
    let (dx, dy) = (tx - cx, ty - cy);
    let len = (dx * dx + dy * dy).sqrt();
    if len <= stop_dist {
        return (cx, cy);
    }
    let travel = (len - stop_dist).min(max_step);
    (cx + dx / len * travel, cy + dy / len * travel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_first_segment_continues_around_the_corner_without_stopping() {
        let path = [(0.5, 0.0, 0.0), (0.5, 7.0, 0.0)];
        assert_eq!(
            sample((0.0, 0.0, 0.0), &path, 1.0 / 7.5),
            ((0.5, 0.5, 0.0), 1)
        );
        assert_eq!(sample((0.0, 0.0, 0.0), &path, 1.0), ((0.5, 7.0, 0.0), 2));
    }

    #[test]
    fn late_viewer_resumes_the_remaining_waypoints() {
        let path = [(3.0, 0.0, 0.0), (3.0, 4.0, 0.0)];
        let (at, next) = sample((0.0, 0.0, 0.0), &path, 0.5);
        assert_eq!(at, (3.0, 0.5, 0.0));
        assert_eq!(&path[next..], &[(3.0, 4.0, 0.0)]);
        assert_eq!(length(at, &path[next..]), 3.5);
    }

    #[test]
    fn progress_follows_turns_and_rejects_positions_off_the_path() {
        let path = [(3.0, 0.0, 0.0), (3.0, 4.0, 0.0)];
        assert_eq!(progress_2d((0.0, 0.0, 0.0), &path, (3.0, 2.0)), Some(5.0));
        assert_eq!(progress_2d((0.0, 0.0, 0.0), &path, (1.5, 2.0)), None);
    }

    #[test]
    fn packed_points_use_signed_destination_offsets() {
        let destination = (10.0, 20.0, 30.0);
        let point = (9.0, 21.0, 29.5);
        let expected = 4 | (0x7fc << 11) | (2 << 22);
        assert_eq!(packed_offset(destination, point), Some(expected));
        assert_eq!(unpacked_point(destination, expected), point);
        assert_eq!(packed_offset(destination, (300.0, 20.0, 30.0)), None);
    }

    #[test]
    fn wire_normalization_preserves_the_destination_and_rejects_overflow() {
        assert_eq!(
            wire_points(&[(0.12, 1.13, 0.0), (5.0, 5.0, 0.0)]),
            Some(vec![(0.0, 1.25, 0.0), (5.0, 5.0, 0.0)])
        );
        assert_eq!(wire_points(&[(0.0, 0.0, 0.0), (300.0, 0.0, 0.0)]), None);
    }

    #[test]
    fn chase_step_closes_to_just_inside_melee() {
        let (x, y) = chase_step(
            0.0,
            0.0,
            20.0,
            0.0,
            crate::constants::speeds::RUN * 4.0,
            4.0,
        );
        assert!(
            (x - 16.0).abs() < 0.01,
            "x {x} expected ~16.0 (4 yd short of the target)"
        );
        assert!(
            (y - 0.0).abs() < 0.01,
            "y {y} expected ~0.0 (straight-line approach)"
        );

        let (x, y) = chase_step(
            0.0,
            0.0,
            100.0,
            0.0,
            crate::constants::speeds::RUN * 4.0,
            4.0,
        );
        assert!(
            (x - 28.0).abs() < 0.01,
            "x {x} expected ~28.0 (one tick of run, capped)"
        );
        assert!((y - 0.0).abs() < 0.01, "y {y} expected ~0.0");

        let (x, y) = chase_step(0.0, 0.0, 3.0, 0.0, 28.0, 4.0);
        assert_eq!((x, y), (0.0, 0.0), "within stop_dist must hold position");

        let (x, y) = chase_step(0.0, 0.0, 4.0, 0.0, 28.0, 4.0);
        assert_eq!(
            (x, y),
            (0.0, 0.0),
            "at the stop_dist boundary must hold position"
        );

        let (x, y) = chase_step(5.0, 5.0, 5.0, 5.0, 28.0, 4.0);
        assert_eq!(
            (x, y),
            (5.0, 5.0),
            "coincident must hold position, never NaN"
        );
        assert!(x.is_finite() && y.is_finite());

        let (x, y) = chase_step(0.0, 0.0, 30.0, 40.0, 100.0, 5.0);
        assert!((x - 27.0).abs() < 0.01, "x {x} expected ~27.0");
        assert!((y - 36.0).abs() < 0.01, "y {y} expected ~36.0");
    }
}
