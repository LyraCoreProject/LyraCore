//! Stable module-to-gateway codes for the Duel lifecycle relay.

/// Authoritative Duel states stored in `game_duel.state`.
pub mod state {
    pub const REQUESTED: u8 = 0;
    pub const COUNTDOWN: u8 = 1;
    pub const ACTIVE: u8 = 2;
}

/// Lifecycle edges stored in `game_duel_event.kind`.
pub mod event_kind {
    pub const REQUESTED: u8 = 0;
    pub const COUNTDOWN: u8 = 1;
    pub const ACTIVE: u8 = 2;
    pub const COMPLETE: u8 = 3;
    pub const OUT_OF_BOUNDS: u8 = 4;
    pub const IN_BOUNDS: u8 = 5;
}

/// Terminal results stored in `game_duel_event.completion_kind`.
pub mod completion_kind {
    pub const INTERRUPTED: u8 = 0;
    pub const WON: u8 = 2;
    pub const FLED: u8 = 3;
}
