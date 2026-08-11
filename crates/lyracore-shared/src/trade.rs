//! The module↔gateway TRADE wire contract (#120) — event-kind codes for the Trade Session status
//! relay. Cross-boundary constants live HERE, both crates import: a module-side renumber becomes a
//! compile-visible edit on the gateway side instead of a runtime drift. Same precedent as
//! `group::event_kind`.

/// Trade-event kinds (`game_trade_event.kind`) — which `SMSG_TRADE_STATUS` variant the gateway
/// relays. One byte per status the module actually emits; NOT the vanilla `TradeStatus`
/// discriminants (the gateway owns that wire mapping, and most vanilla variants are never
/// module-emitted).
pub mod event_kind {
    /// You are proposed a trade (`other_guid` = the initiator) → `TradeStatus::BeginTrade`.
    /// The client answers with `CMSG_BEGIN_TRADE`, which opens the window on both sides.
    pub const BEGIN_TRADE: u8 = 0;
    /// The proposal was answered — both parties' windows open → `TradeStatus::OpenWindow`.
    pub const OPEN_WINDOW: u8 = 1;
    /// The Trade Session ended without a Trade Commit (explicit cancel, logout) →
    /// `TradeStatus::TradeCanceled`.
    pub const TRADE_CANCELED: u8 = 2;
    /// Initiator refusal: you or the target are already in a Trade Session → `TradeStatus::Busy`.
    pub const BUSY: u8 = 3;
    /// Initiator refusal: no such partner (absent, mid-transfer, not a player, or yourself) →
    /// `TradeStatus::NoTarget`.
    pub const NO_TARGET: u8 = 4;
    /// Initiator refusal: partner beyond the 10 yd interaction range (or another partition) →
    /// `TradeStatus::TargetToFar` (the wire library's spelling).
    pub const TARGET_TO_FAR: u8 = 5;
    /// Initiator refusal: cross-faction trade → `TradeStatus::WrongFaction`.
    pub const WRONG_FACTION: u8 = 6;
    /// Initiator refusal: you are dead → `TradeStatus::YouDead`.
    pub const YOU_DEAD: u8 = 7;
    /// Initiator refusal: the target is dead → `TradeStatus::TargetDead`.
    pub const TARGET_DEAD: u8 = 8;
}
