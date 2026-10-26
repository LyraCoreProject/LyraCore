//! Build 5875 MovementInfo flag bits.

pub const FORWARD: u32 = 0x1;
pub const BACKWARD: u32 = 0x2;
pub const STRAFE_LEFT: u32 = 0x4;
pub const STRAFE_RIGHT: u32 = 0x8;
pub const JUMPING: u32 = 0x2000;
pub const FALLING_FAR: u32 = 0x4000;
// SWIMMING also applies at the surface; combine it with the liquid height.
pub const SWIMMING: u32 = 0x0020_0000;
pub const ON_TRANSPORT: u32 = 0x0200_0000;

/// Forward, backward and strafe: the movement a root forbids.
pub const STEERING: u32 = FORWARD | BACKWARD | STRAFE_LEFT | STRAFE_RIGHT;
/// Steering plus jumping and falling far: "any movement" for a break-on-move aura or a swing.
pub const MOVING: u32 = STEERING | JUMPING | FALLING_FAR;
