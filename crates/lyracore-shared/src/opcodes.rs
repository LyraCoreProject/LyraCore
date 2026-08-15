//! Opcode numbers for the slice. Values are the well-known vanilla 1.12.1 (build 5875)
//! constants. The gateway uses `wow_world_messages` for the actual byte layouts; these
//! numbers are the gateway's inbound dispatch keys (matched directly in `world::dispatch`).
//!
//! Cross-check against `wow_world_messages` opcode enums before wiring a new handler — the codec
//! is the source of truth for layouts; this module is the source of truth for *which*
//! opcodes the slice handles.

/// Logon-server commands (port 3724). 1-byte command tag, not the world framing.
pub mod logon {
    pub const CMD_AUTH_LOGON_CHALLENGE: u8 = 0x00;
    pub const CMD_AUTH_LOGON_PROOF: u8 = 0x01;
    pub const CMD_REALM_LIST: u8 = 0x10;
}

/// World-server opcodes (port 8085). SMSG headers are 4 bytes, CMSG headers 6 bytes.
pub mod world {
    // --- auth handshake ---
    pub const SMSG_AUTH_CHALLENGE: u16 = 0x1EC;
    pub const CMSG_AUTH_SESSION: u32 = 0x1ED;
    pub const SMSG_AUTH_RESPONSE: u16 = 0x1EE;

    // --- character select / login ---
    pub const CMSG_CHAR_ENUM: u32 = 0x037;
    pub const SMSG_CHAR_ENUM: u16 = 0x03B;
    pub const CMSG_PLAYER_LOGIN: u32 = 0x03D;
    pub const CMSG_LOGOUT_REQUEST: u32 = 0x04B; // stretch
    pub const SMSG_LOGOUT_RESPONSE: u16 = 0x04C; // stretch
    pub const SMSG_LOGOUT_COMPLETE: u16 = 0x04D; // stretch

    // --- /played ---
    pub const CMSG_PLAYED_TIME: u32 = 0x1CC;
    pub const SMSG_PLAYED_TIME: u16 = 0x1CD;

    // --- login sequence (S->C) ---
    pub const SMSG_LOGIN_SETTIMESPEED: u16 = 0x042;
    pub const SMSG_TUTORIAL_FLAGS: u16 = 0x0FD;
    pub const SMSG_INITIALIZE_FACTIONS: u16 = 0x122;
    pub const SMSG_ACTION_BUTTONS: u16 = 0x129;
    pub const SMSG_INITIAL_SPELLS: u16 = 0x12A;
    pub const SMSG_BINDPOINTUPDATE: u16 = 0x155;
    pub const SMSG_ACCOUNT_DATA_TIMES: u16 = 0x209;
    pub const SMSG_SET_REST_START: u16 = 0x21E;
    pub const SMSG_LOGIN_VERIFY_WORLD: u16 = 0x236;

    // --- world state / object update ---
    pub const SMSG_UPDATE_OBJECT: u16 = 0x0A9;
    pub const SMSG_DESTROY_OBJECT: u16 = 0x0AA;
    pub const SMSG_COMPRESSED_UPDATE_OBJECT: u16 = 0x1F6; // deferred
}

/// Guild opcodes. Verified against `wow_world_messages`' vanilla opcode table rather than quoted
/// from memory — LEAVE / REMOVE / DISBAND sit at 0x08D / 0x08E / 0x08F and are easy to transpose.
/// Later guild slices append the verbs they handle.
pub mod guild {
    pub const CMSG_GUILD_QUERY: u32 = 0x054;
    pub const SMSG_GUILD_QUERY_RESPONSE: u16 = 0x055;
    pub const CMSG_GUILD_CREATE: u32 = 0x081;
    pub const CMSG_GUILD_INFO: u32 = 0x087;
    pub const SMSG_GUILD_INFO: u16 = 0x088;
    pub const SMSG_GUILD_COMMAND_RESULT: u16 = 0x093;
    pub const CMSG_GUILD_INVITE: u32 = 0x082;
    pub const SMSG_GUILD_INVITE: u16 = 0x083;
    pub const CMSG_GUILD_ACCEPT: u32 = 0x084;
    pub const CMSG_GUILD_DECLINE: u32 = 0x085;
    pub const SMSG_GUILD_EVENT: u16 = 0x092;
}

/// `MSG_MOVE_*` opcodes. These flow **both** directions: the client sends its own movement,
/// the server relays the same opcode to nearby observers. The slice handles the subset that
/// looks like real movement (forward/stop/turn/facing/heartbeat).
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
    pub const MSG_MOVE_SET_RUN_MODE: u32 = 0x0C2;
    pub const MSG_MOVE_SET_WALK_MODE: u32 = 0x0C3;
    pub const MSG_MOVE_FALL_LAND: u32 = 0x0C9;
    pub const MSG_MOVE_START_SWIM: u32 = 0x0CA;
    pub const MSG_MOVE_STOP_SWIM: u32 = 0x0CB;
    pub const MSG_MOVE_SET_FACING: u32 = 0x0DA;
    pub const MSG_MOVE_HEARTBEAT: u32 = 0x0EE;

    /// The movement opcodes the slice routes to `movement_update`.
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
        MSG_MOVE_SET_RUN_MODE,
        MSG_MOVE_SET_WALK_MODE,
        MSG_MOVE_FALL_LAND,
        MSG_MOVE_START_SWIM,
        MSG_MOVE_STOP_SWIM,
        MSG_MOVE_SET_FACING,
        MSG_MOVE_HEARTBEAT,
    ];

    /// True if `opcode` is a movement opcode the slice relays.
    pub fn is_slice_move(opcode: u32) -> bool {
        SLICE_MOVE_OPCODES.contains(&opcode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slice_move_membership() {
        // Every one of the 17 SLICE_MOVE_OPCODES entries is recognized (not just a hand-picked subset).
        for &opcode in movement::SLICE_MOVE_OPCODES {
            assert!(
                movement::is_slice_move(opcode),
                "0x{opcode:03X} should be a slice move opcode"
            );
        }
        assert_eq!(movement::SLICE_MOVE_OPCODES.len(), 17);
        // Non-movement opcodes (world handshake/object codes) are never mistaken for movement.
        assert!(!movement::is_slice_move(world::CMSG_PLAYER_LOGIN));
        assert!(!movement::is_slice_move(world::SMSG_UPDATE_OBJECT as u32));
        assert!(!movement::is_slice_move(world::CMSG_AUTH_SESSION));
        assert!(!movement::is_slice_move(0)); // opcode 0 is never valid
        assert!(!movement::is_slice_move(u32::MAX)); // an out-of-range sentinel
    }
}
