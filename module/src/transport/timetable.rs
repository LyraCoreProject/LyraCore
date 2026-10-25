//! Transport timetables, following CMaNGOS `GenerateWaypoints` and `CalculateSegmentPos`.

use std::f32::consts::{PI, TAU};

const TELEPORT_FLAG: i32 = 1;
const STOP_FLAG: i32 = 2;
// The client measures a segment with 20 chords; CMaNGOS uses 3.
const ARC_STEPS: u32 = 20;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PathPoint {
    pub(crate) map_id: u32,
    pub(crate) position: [f32; 3],
    pub(crate) flags: i32,
    pub(crate) delay_ms: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Pose {
    pub(crate) map_id: u32,
    pub(crate) position: [f32; 3],
    pub(crate) orientation: f32,
}

impl Pose {
    /// World pose of a Passenger standing at `offset` (x, y, z, orientation) on this Transport.
    pub(crate) fn carry(&self, offset: [f32; 4]) -> [f32; 4] {
        let (sin, cos) = self.orientation.sin_cos();
        [
            self.position[0] + offset[0] * cos - offset[1] * sin,
            self.position[1] + offset[1] * cos + offset[0] * sin,
            self.position[2] + offset[2],
            (self.orientation + offset[3]).rem_euclid(TAU),
        ]
    }
}

#[derive(Clone, Debug)]
struct Frame {
    map_id: u32,
    position: [f32; 3],
    orientation: f32,
    stop: bool,
    teleport: bool,
    delay_s: f32,
    leg: usize,
    leg_index: usize,
    dist_from_prev: f32,
    next_dist: f32,
    dist_since_stop: f32,
    dist_until_stop: f32,
    time_from: f32,
    time_to: f32,
    departure_ms: u32,
    next_arrival_ms: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct Timetable {
    frames: Vec<Frame>,
    legs: Vec<Vec<[f32; 3]>>,
    period_ms: u32,
    speed: f32,
    accel: f32,
    accel_time: f32,
    accel_dist: f32,
}

impl Timetable {
    /// `points` is one TaxiPathNode path in node order; `period_ms` is the client's cycle.
    pub(crate) fn build(
        points: &[PathPoint],
        speed: u32,
        accel: u32,
        period_ms: u32,
    ) -> Option<Self> {
        if points.len() < 3 || speed == 0 || accel == 0 || period_ms == 0 {
            return None;
        }
        let (speed, accel) = (speed as f32, accel as f32);
        let mut frames: Vec<Frame> = Vec::new();
        let mut skip = false;
        for index in 1..points.len() - 1 {
            if std::mem::take(&mut skip) {
                continue;
            }
            let (point, next) = (points[index], points[index + 1]);
            if point.flags & TELEPORT_FLAG != 0 || point.map_id != next.map_id {
                frames.last_mut()?.teleport = true;
                skip = true;
                continue;
            }
            frames.push(Frame {
                map_id: point.map_id,
                position: point.position,
                orientation: heading(sub(next.position, points[index - 1].position)),
                stop: point.flags & STOP_FLAG != 0,
                teleport: false,
                delay_s: point.delay_ms as f32 / 1000.0,
                leg: 0,
                leg_index: 0,
                dist_from_prev: 0.0,
                next_dist: 0.0,
                dist_since_stop: 0.0,
                dist_until_stop: 0.0,
                time_from: 0.0,
                time_to: 0.0,
                departure_ms: 0,
                next_arrival_ms: 0,
            });
        }
        frames.last_mut()?.teleport = true;
        let count = frames.len();

        let mut legs: Vec<Vec<[f32; 3]>> = vec![Vec::new()];
        for index in 0..count {
            if index > 0 && frames[index - 1].teleport {
                legs.push(Vec::new());
            }
            let leg = legs.len() - 1;
            frames[index].leg = leg;
            frames[index].leg_index = legs[leg].len();
            legs[leg].push(frames[index].position);
        }
        for frame in &mut frames {
            if frame.leg_index > 0 {
                frame.dist_from_prev = segment_length(&legs[frame.leg], frame.leg_index - 1);
            }
        }
        for index in 0..count {
            frames[index].next_dist = if frames[index].teleport || index + 1 == count {
                0.0
            } else {
                frames[index + 1].dist_from_prev
            };
        }

        let first_stop = frames.iter().position(|frame| frame.stop).unwrap_or(0);
        let last_stop = frames.iter().rposition(|frame| frame.stop).unwrap_or(0);
        let mut distance = 0.0;
        for offset in 0..count {
            let index = (offset + last_stop) % count;
            distance = if frames[index].stop || index == last_stop {
                0.0
            } else {
                distance + frames[index].dist_from_prev
            };
            frames[index].dist_since_stop = distance;
        }
        let mut distance = 0.0;
        for offset in (0..count).rev() {
            let index = (offset + first_stop) % count;
            distance += frames[(index + 1) % count].dist_from_prev;
            frames[index].dist_until_stop = distance;
            if frames[index].stop || index == first_stop {
                distance = 0.0;
            }
        }

        let accel_time = speed / accel;
        let accel_dist = 0.5 * speed * accel_time;
        for frame in &mut frames {
            let (since, until) = (frame.dist_since_stop, frame.dist_until_stop);
            frame.time_to = if since + until < 2.0 * accel_dist {
                if since < until {
                    2.0 * ((until + since) / accel).sqrt() - (2.0 * since / accel).sqrt()
                } else {
                    (2.0 * until / accel).sqrt()
                }
            } else if since < accel_dist {
                (until + since) / speed + accel_time - (2.0 * since / accel).sqrt()
            } else if until < accel_dist {
                (2.0 * until / accel).sqrt()
            } else {
                until / speed + 0.5 * accel_time
            };
        }
        let mut segment_time = 0.0;
        for offset in 0..count {
            let index = (offset + last_stop) % count;
            if frames[index].stop || index == last_stop {
                segment_time = frames[index].time_to;
            }
            frames[index].time_from = segment_time - frames[index].time_to;
        }

        let mut elapsed = 0.0f32;
        if frames[0].stop {
            elapsed = frames[0].delay_s;
            frames[0].departure_ms = seconds_to_ms(elapsed);
        }
        for index in 1..count {
            elapsed += frames[index - 1].time_to;
            let arrival_ms = if frames[index].stop {
                let arrival_ms = seconds_to_ms(elapsed);
                elapsed += frames[index].delay_s;
                frames[index].departure_ms = seconds_to_ms(elapsed);
                arrival_ms
            } else {
                elapsed -= frames[index].time_to;
                let arrival_ms = seconds_to_ms(elapsed);
                frames[index].departure_ms = arrival_ms;
                arrival_ms
            };
            frames[index - 1].next_arrival_ms = arrival_ms;
        }
        let last = &mut frames[count - 1];
        last.next_arrival_ms = last.departure_ms;
        last.departure_ms = period_ms;

        Some(Self {
            frames,
            legs,
            period_ms,
            speed,
            accel,
            accel_time,
            accel_dist,
        })
    }

    pub(crate) fn pose(&self, cycle_ms: u32) -> Pose {
        let cycle_ms = cycle_ms % self.period_ms;
        let index = self
            .frames
            .iter()
            .position(|frame| cycle_ms < frame.next_arrival_ms)
            .unwrap_or(self.frames.len() - 1);
        let frame = &self.frames[index];
        if cycle_ms < frame.departure_ms || frame.next_dist <= 0.0 {
            return Pose {
                map_id: frame.map_id,
                position: frame.position,
                orientation: frame.orientation,
            };
        }
        let since_departure = (cycle_ms - frame.departure_ms) as f32 / 1000.0;
        let time_since_stop = frame.time_from + since_departure;
        let time_until_stop = frame.time_to - since_departure;
        let travelled = if time_since_stop < time_until_stop {
            self.distance_after(time_since_stop) - frame.dist_since_stop
        } else {
            frame.dist_until_stop - self.distance_after(time_until_stop)
        };
        let leg = &self.legs[frame.leg];
        let t = (travelled / frame.next_dist).clamp(0.0, 1.0);
        Pose {
            map_id: frame.map_id,
            position: evaluate(leg, frame.leg_index, t, catmull_rom_weights),
            orientation: heading(evaluate(leg, frame.leg_index, t, catmull_rom_slopes)),
        }
    }

    fn distance_after(&self, seconds: f32) -> f32 {
        if seconds < self.accel_time {
            0.5 * self.accel * seconds * seconds
        } else {
            self.accel_dist + (seconds - self.accel_time) * self.speed
        }
    }
}

fn seconds_to_ms(seconds: f32) -> u32 {
    (seconds * 1000.0).max(0.0) as u32
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// Transport models face away from their direction of travel.
fn heading(direction: [f32; 3]) -> f32 {
    (direction[1].atan2(direction[0]) + PI).rem_euclid(TAU)
}

fn control(leg: &[[f32; 3]], index: isize) -> [f32; 3] {
    let last = leg.len() - 1;
    match usize::try_from(index) {
        Err(_) => {
            let (first, second) = (leg[0], leg[1.min(last)]);
            [
                2.0 * first[0] - second[0],
                2.0 * first[1] - second[1],
                2.0 * first[2] - second[2],
            ]
        }
        Ok(index) => leg[index.min(last)],
    }
}

fn evaluate(leg: &[[f32; 3]], segment: usize, t: f32, weights: fn(f32) -> [f32; 4]) -> [f32; 3] {
    let weights = weights(t);
    let mut out = [0.0; 3];
    for (offset, weight) in weights.into_iter().enumerate() {
        let point = control(leg, segment as isize - 1 + offset as isize);
        for axis in 0..3 {
            out[axis] += point[axis] * weight;
        }
    }
    out
}

fn segment_length(leg: &[[f32; 3]], segment: usize) -> f32 {
    let mut previous = evaluate(leg, segment, 0.0, catmull_rom_weights);
    let mut length = 0.0f64;
    for step in 1..=ARC_STEPS {
        let point = evaluate(
            leg,
            segment,
            step as f32 / ARC_STEPS as f32,
            catmull_rom_weights,
        );
        let delta = sub(point, previous);
        length += f64::from(delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
        previous = point;
    }
    length as f32
}

fn catmull_rom_weights(t: f32) -> [f32; 4] {
    let (t2, t3) = (t * t, t * t * t);
    [
        -0.5 * t3 + t2 - 0.5 * t,
        1.5 * t3 - 2.5 * t2 + 1.0,
        -1.5 * t3 + 2.0 * t2 + 0.5 * t,
        0.5 * t3 - 0.5 * t2,
    ]
}

fn catmull_rom_slopes(t: f32) -> [f32; 4] {
    let t2 = t * t;
    [
        -1.5 * t2 + 2.0 * t - 0.5,
        4.5 * t2 - 5.0 * t,
        -4.5 * t2 + 4.0 * t + 0.5,
        1.5 * t2 - t,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(map_id: u32, x: f32, flags: i32, delay_ms: u32) -> PathPoint {
        PathPoint {
            map_id,
            position: [x, 0.0, 0.0],
            flags,
            delay_ms,
        }
    }

    /// Stops at x=0 and x=300 for 10 s each; 10 yd/s with 5 yd/s² reaches cruise in 2 s and 10 yd.
    fn ferry() -> Timetable {
        let points = [
            point(1, -100.0, 0, 0),
            point(1, 0.0, STOP_FLAG, 10_000),
            point(1, 100.0, 0, 0),
            point(1, 200.0, 0, 0),
            point(1, 300.0, STOP_FLAG, 10_000),
            point(1, 400.0, 0, 0),
            point(1, 500.0, 0, 0),
        ];
        Timetable::build(&points, 10, 5, 70_000).expect("valid path")
    }

    fn x_at(timetable: &Timetable, cycle_ms: u32) -> f32 {
        timetable.pose(cycle_ms).position[0]
    }

    #[test]
    fn a_transport_waits_at_each_stop_for_its_delay() {
        let ferry = ferry();
        assert_eq!(x_at(&ferry, 0), 0.0);
        assert_eq!(x_at(&ferry, 9_999), 0.0);
        assert_eq!(x_at(&ferry, 42_000), 300.0);
        assert_eq!(x_at(&ferry, 51_999), 300.0);
    }

    #[test]
    fn a_transport_accelerates_cruises_and_brakes_between_stops() {
        let ferry = ferry();
        assert!((x_at(&ferry, 11_000) - 2.5).abs() < 0.01);
        assert!((x_at(&ferry, 22_000) - 110.0).abs() < 0.01);
        assert!((x_at(&ferry, 41_000) - 297.5).abs() < 0.01);
    }

    #[test]
    fn the_last_frame_holds_until_the_period_wraps() {
        let ferry = ferry();
        assert_eq!(x_at(&ferry, 66_000), 400.0);
        assert_eq!(x_at(&ferry, 75_000), 0.0);
    }

    #[test]
    fn a_transport_faces_away_from_its_travel() {
        let ferry = ferry();
        assert!((ferry.pose(22_000).orientation - PI).abs() < 1e-4);
        assert!((ferry.pose(45_000).orientation - PI).abs() < 1e-4);
    }

    #[test]
    fn a_map_change_starts_a_new_leg_on_the_next_map() {
        let points = [
            point(0, -100.0, 0, 0),
            point(0, 0.0, STOP_FLAG, 10_000),
            point(0, 100.0, 0, 0),
            point(0, 200.0, 0, 0),
            point(0, 300.0, 0, 0),
            point(1, 5_000.0, 0, 0),
            point(1, 5_100.0, 0, 0),
            point(1, 5_200.0, STOP_FLAG, 10_000),
            point(1, 5_300.0, 0, 0),
            point(1, 5_400.0, 0, 0),
        ];
        let timetable = Timetable::build(&points, 10, 5, 100_000).expect("valid path");
        assert_eq!(timetable.pose(0).map_id, 0);
        let docked = (0..100_000)
            .step_by(500)
            .map(|cycle_ms| timetable.pose(cycle_ms))
            .find(|pose| pose.map_id == 1 && pose.position[0] == 5_200.0)
            .expect("the second stop is reached");
        assert_eq!(docked.position, [5_200.0, 0.0, 0.0]);
    }

    #[test]
    fn a_passenger_offset_turns_with_the_transport() {
        let pose = Pose {
            map_id: 1,
            position: [100.0, 200.0, 5.0],
            orientation: PI / 2.0,
        };
        let [x, y, z, o] = pose.carry([2.0, 1.0, 3.0, 0.5]);
        assert!((x - 99.0).abs() < 1e-4 && (y - 202.0).abs() < 1e-4);
        assert_eq!(z, 8.0);
        assert!((o - (PI / 2.0 + 0.5)).abs() < 1e-5);
    }

    #[test]
    fn a_path_without_travel_points_has_no_timetable() {
        let points = [point(1, 0.0, 0, 0), point(1, 1.0, 0, 0)];
        assert!(Timetable::build(&points, 10, 5, 1_000).is_none());
        assert!(Timetable::build(&[point(1, 0.0, 0, 0); 4], 0, 5, 1_000).is_none());
    }
}
