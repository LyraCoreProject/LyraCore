// MSG_MOVE opcodes travel in both directions.
pub mod movement {
    pub const MSG_MOVE_START_FORWARD: u32 = 0x0B5;
    pub const MSG_MOVE_START_BACKWARD: u32 = 0x0B6;
    pub const MSG_MOVE_STOP: u32 = 0x0B7;
    pub const MSG_MOVE_START_STRAFE_LEFT: u32 = 0x0B8;
    pub const MSG_MOVE_START_STRAFE_RIGHT: u32 = 0x0B9;
    pub const MSG_MOVE_STOP_STRAFE: u32 = 0x0BA;
    pub const MSG_MOVE_JUMP: u32 = 0x0BB;
    pub const MSG_MOVE_START_TURN_LEFT: u32 = 0x0BC;
    pub const MSG_MOVE_START_TURN_RIGHT: u32 = 0x0BD;
    pub const MSG_MOVE_STOP_TURN: u32 = 0x0BE;
    pub const MSG_MOVE_START_PITCH_UP: u32 = 0x0BF;
    pub const MSG_MOVE_START_PITCH_DOWN: u32 = 0x0C0;
    pub const MSG_MOVE_STOP_PITCH: u32 = 0x0C1;
    pub const MSG_MOVE_SET_RUN_MODE: u32 = 0x0C2;
    pub const MSG_MOVE_SET_WALK_MODE: u32 = 0x0C3;
    pub const MSG_MOVE_FALL_LAND: u32 = 0x0C9;
    pub const MSG_MOVE_START_SWIM: u32 = 0x0CA;
    pub const MSG_MOVE_STOP_SWIM: u32 = 0x0CB;
    pub const MSG_MOVE_SET_FACING: u32 = 0x0DA;
    pub const MSG_MOVE_SET_PITCH: u32 = 0x0DB;
    pub const MSG_MOVE_ROOT: u32 = 0x0EC;
    pub const MSG_MOVE_UNROOT: u32 = 0x0ED;
    pub const MSG_MOVE_HEARTBEAT: u32 = 0x0EE;
    pub const MSG_MOVE_WATER_WALK: u32 = 0x2B1;

    pub const SLICE_MOVE_OPCODES: &[u32] = &[
        MSG_MOVE_START_FORWARD,
        MSG_MOVE_START_BACKWARD,
        MSG_MOVE_STOP,
        MSG_MOVE_START_STRAFE_LEFT,
        MSG_MOVE_START_STRAFE_RIGHT,
        MSG_MOVE_STOP_STRAFE,
        MSG_MOVE_JUMP,
        MSG_MOVE_START_TURN_LEFT,
        MSG_MOVE_START_TURN_RIGHT,
        MSG_MOVE_STOP_TURN,
        MSG_MOVE_START_PITCH_UP,
        MSG_MOVE_START_PITCH_DOWN,
        MSG_MOVE_STOP_PITCH,
        MSG_MOVE_SET_RUN_MODE,
        MSG_MOVE_SET_WALK_MODE,
        MSG_MOVE_FALL_LAND,
        MSG_MOVE_START_SWIM,
        MSG_MOVE_STOP_SWIM,
        MSG_MOVE_SET_FACING,
        MSG_MOVE_SET_PITCH,
        MSG_MOVE_HEARTBEAT,
    ];

    /// Observer relays recorded after a client acknowledgement; the client never sends them.
    pub const ACKNOWLEDGED_MOVE_OPCODES: &[u32] =
        &[MSG_MOVE_ROOT, MSG_MOVE_UNROOT, MSG_MOVE_WATER_WALK];

    pub fn is_slice_move(opcode: u32) -> bool {
        SLICE_MOVE_OPCODES.contains(&opcode)
    }

    pub fn is_relayed_move(opcode: u32) -> bool {
        is_slice_move(opcode) || ACKNOWLEDGED_MOVE_OPCODES.contains(&opcode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slice_move_membership() {
        for &opcode in movement::SLICE_MOVE_OPCODES {
            assert!(
                movement::is_slice_move(opcode),
                "0x{opcode:03X} should be a slice move opcode"
            );
        }
        assert_eq!(movement::SLICE_MOVE_OPCODES.len(), 21);
        for &opcode in movement::ACKNOWLEDGED_MOVE_OPCODES {
            assert!(movement::is_relayed_move(opcode));
            assert!(!movement::is_slice_move(opcode));
        }
        for opcode in [0x03D, 0x0A9, 0x1ED] {
            assert!(
                !movement::is_slice_move(opcode),
                "0x{opcode:03X} is not a move"
            );
        }
        assert!(!movement::is_slice_move(0));
        assert!(!movement::is_slice_move(u32::MAX));
    }
}
