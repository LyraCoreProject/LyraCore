//! Addon⇄server message bridge (work-item 184) — the UI↔server RPC channel custom features ride.
//!
//! Client→server: the 1.12 client's `SendAddonMessage` is `CMSG_MESSAGECHAT` with language
//! `LANG_ADDON` (0xFFFFFFFF) and text `"<prefix>\t<message>"`. The GATEWAY intercepts those frames
//! raw (gtker's Language enum can't decode 0xFFFFFFFF), parses the `STC` envelope, and calls the
//! [`client_command`] reducer AS the player — a command handler has exactly a reducer's authority.
//!
//! Server→client: [`send`] inserts a [`AddonMessage`] row; the gateway relays it (COORDINATOR
//! connection — the 279 delivery law: addon UI state must survive fat transactions) as an
//! addon-language whisper the client surfaces to addons as `CHAT_MSG_ADDON`.
//!
//! Dispatch is a plain match in [`dispatch`] for now — a feature adds one arm next to its handler
//! fn. The `game_client_command!` build.rs marker (the 143-substrate ideal from the 184 plan) can
//! replace the match when a second package needs registration without touching this file.

use spacetimedb::{
    table, Identity, ReducerContext, Table, Timestamp,
};

use crate::game_character;

/// One server→client addon message: relayed by the gateway as an addon-language whisper, arriving
/// in the recipient's addons as `CHAT_MSG_ADDON` with `prefix == "STC"`. TTL-reaped with the other
/// event tables (same `created_at` shape). [event]
#[table(accessor = game_addon_message, public)]
pub struct AddonMessage {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub recipient_identity: Identity,
    /// Command name, e.g. `"pong"` or `"event.progress"` — the addon routes on it.
    pub cmd: String,
    /// Command payload, a command-defined micro-format (no JSON — 255-byte wire budget).
    pub payload: String,
    pub created_at: Timestamp,
}

/// Queue an addon message for `character_guid`'s live session. The recipient identity comes off
/// the durable character row (its owner binding) — a logged-out recipient's row is inserted and
/// simply reaped unread (the 1s event TTL), which is the right semantics for UI state.
pub(crate) fn send(ctx: &ReducerContext, character_guid: u64, cmd: &str, payload: &str) {
    let Some(c) = ctx.db.game_character().guid().find(character_guid) else {
        return;
    };
    ctx.db.game_addon_message().insert(AddonMessage {
        id: 0,
        recipient_identity: c.owner_identity,
        cmd: cmd.to_string(),
        payload: payload.to_string(),
        created_at: ctx.timestamp,
    });
}

/// The shared core behind [`client_command`] and its gateway twin `gw_client_command`:
/// dispatch with the sender already resolved to a guid.
pub(crate) fn apply_client_command(
    ctx: &ReducerContext,
    character_guid: u64,
    cmd: &str,
    payload: &str,
) -> Result<(), String> {
    dispatch(ctx, character_guid, cmd, payload);
    Ok(())
}

/// Command dispatch — one arm per command, handlers live next to their features.
fn dispatch(ctx: &ReducerContext, character_guid: u64, cmd: &str, payload: &str) {
    match cmd {
        // The 184 acceptance round-trip: `STC ping` → `STC pong` (payload echoed).
        "ping" => send(ctx, character_guid, "pong", payload),
        other => {
            spacetimedb::log::info!(
                "bridge: unknown client command {other:?} from {character_guid} (dropped)"
            );
        }
    }
}
