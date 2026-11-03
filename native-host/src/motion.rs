use std::time::{Duration, Instant};

include!("native_motion_opcodes.rs");
// MovementInfo flag for a Passenger; the Module handles boarding.
const ON_TRANSPORT: u32 = 0x0200_0000;
// MovementInfo: flags u32, time u32, x, y, z and orientation f32, then fall time u32.
const MOVEMENT_INFO_BYTES: usize = 28;
// Optional MovementInfo tails: swim pitch, jump velocity and spline elevation, in bytes.
const MOVEMENT_TAILS: [(u32, usize); 3] = [(0x0020_0000, 4), (0x2000, 16), (0x0400_0000, 4)];
pub(super) const HEARTBEAT: u16 = 0x00ee;
const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
// The Module's persist drift; the melee leeway assumes its position trails by no more.
const SAMPLE_DRIFT_YD: f32 = 4.0;

pub(super) fn coalescible(opcode: u16) -> bool {
    COALESCIBLE_MOTION_OPCODES.contains(&opcode)
}

/// The fields of a client movement message that the Host checks before relaying it.
#[derive(Clone, Copy)]
pub(super) struct Pose {
    opcode: u16,
    time_ms: u32,
    x: f32,
    y: f32,
}

/// A movement message the Host may relay: on foot, finite and as long as its flags require.
pub(super) fn pose(opcode: u32, body: &[u8]) -> Option<Pose> {
    let opcode = u16::try_from(opcode).ok()?;
    if !CLIENT_MOTION_OPCODES.contains(&opcode) || body.len() < MOVEMENT_INFO_BYTES {
        return None;
    }
    let word = |at: usize| u32::from_le_bytes(body[at..at + 4].try_into().unwrap_or_default());
    let float = |at: usize| f32::from_bits(word(at));
    let flags = word(0);
    let length = MOVEMENT_TAILS
        .iter()
        .filter(|(flag, _)| flags & flag != 0)
        .map(|(_, bytes)| bytes)
        .sum::<usize>()
        + MOVEMENT_INFO_BYTES;
    if flags & ON_TRANSPORT != 0
        || body.len() != length
        || (8..24).step_by(4).any(|at| !float(at).is_finite())
    {
        return None;
    }
    Some(Pose {
        opcode,
        time_ms: word(4),
        x: float(8),
        y: float(12),
    })
}

impl Pose {
    pub(super) fn opcode(&self) -> u16 {
        self.opcode
    }

    pub(super) fn position(&self) -> (f32, f32) {
        (self.x, self.y)
    }
}

/// One connection's movement as the Host last accepted and sampled it.
#[derive(Default)]
pub(super) struct Mover {
    time_ms: Option<u32>,
    sampled: Option<(Instant, f32, f32)>,
}

impl Mover {
    /// Whether the pose is newer than the last accepted one, as the Module's Gate requires.
    pub(super) fn follows(&self, pose: &Pose) -> bool {
        match self.time_ms {
            Some(last) if last != 0 && pose.time_ms != 0 => {
                (pose.time_ms.wrapping_sub(last) as i32) > 0
            }
            _ => true,
        }
    }

    /// Records a pose; returns whether the Module should receive it as a sample.
    pub(super) fn accept(&mut self, pose: Pose, now: Instant) -> bool {
        self.time_ms = Some(pose.time_ms);
        let due = pose.opcode != HEARTBEAT
            || self.sampled.is_none_or(|(at, x, y)| {
                now.duration_since(at) >= SAMPLE_INTERVAL
                    || (pose.x - x).powi(2) + (pose.y - y).powi(2) >= SAMPLE_DRIFT_YD.powi(2)
            });
        if due {
            self.sampled = Some((now, pose.x, pose.y));
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn movement(opcode: u32, flags: u32, time_ms: u32, x: f32) -> (u32, Vec<u8>) {
        let mut body = Vec::new();
        for word in [flags, time_ms, x.to_bits(), 0, 0, 0, 0] {
            body.extend(word.to_le_bytes());
        }
        (opcode, body)
    }

    fn accepted(mover: &mut Mover, (opcode, body): (u32, Vec<u8>), now: Instant) -> Option<bool> {
        let pose = pose(opcode, &body)?;
        mover.follows(&pose).then(|| mover.accept(pose, now))
    }

    #[test]
    fn passengers_short_bodies_and_other_opcodes_stay_with_the_module() {
        let (_, body) = movement(0x0ee, ON_TRANSPORT, 1, 0.0);
        assert!(pose(0x0ee, &body).is_none());
        assert!(pose(0x0ee, &body[..20]).is_none());
        let (_, body) = movement(0x0ee, 0, 1, f32::NAN);
        assert!(pose(0x0ee, &body).is_none());
        let (_, body) = movement(0x0c7, 0, 1, 0.0);
        assert!(pose(0x0c7, &body).is_none());
        let (_, mut body) = movement(0x0bb, 0x2000, 1, 0.0);
        assert!(pose(0x0bb, &body).is_none());
        body.extend([0; 16]);
        assert!(pose(0x0bb, &body).is_some());
        body.push(0);
        assert!(pose(0x0bb, &body).is_none());
    }

    #[test]
    fn heartbeats_are_sampled_about_once_a_second_and_other_movement_always() {
        let start = Instant::now();
        let mut mover = Mover::default();
        let at = |ms| start + Duration::from_millis(ms);
        assert_eq!(
            accepted(&mut mover, movement(0x0ee, 0, 100, 0.0), at(0)),
            Some(true)
        );
        assert_eq!(
            accepted(&mut mover, movement(0x0ee, 0, 600, 3.0), at(500)),
            Some(false)
        );
        assert_eq!(
            accepted(&mut mover, movement(0x0bb, 0, 700, 3.5), at(600)),
            Some(true)
        );
        assert_eq!(
            accepted(&mut mover, movement(0x0da, 0, 800, 3.5), at(700)),
            Some(true)
        );
        assert_eq!(
            accepted(&mut mover, movement(0x0ee, 0, 1200, 6.0), at(1100)),
            Some(false)
        );
        assert_eq!(
            accepted(&mut mover, movement(0x0ee, 0, 1700, 12.0), at(1600)),
            Some(true)
        );
        assert_eq!(
            accepted(&mut mover, movement(0x0ee, 0, 1700, 13.0), at(1700)),
            None
        );
    }
}
