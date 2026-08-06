//! Whisper refusals (issue #22, WHISPER slice) — the exact text a rejected `/w` produces, shared
//! because the whisper slice gave it TWO producers.
//!
//! Before #22 the module's `send_whisper` was the only party that could refuse a whisper: it
//! resolved the typed name against `game_character` and read the target's `online` flag, both on the
//! calling database. That is precisely why a whisper could not cross a shard boundary — a target
//! standing in Deadmines has no character row on the open world's database — so on a multi-database
//! gateway both gates now run in the GATEWAY, across every connected shard, and the module's
//! realm-core reducer trusts their answer (it has neither table to re-check).
//!
//! Two producers of one refusal is a drift risk, and this is the cheap fence: the strings live here,
//! in the one crate both sides compile, so a reword is a single compile-visible edit rather than a
//! silent divergence between "the whisper you typed on a sharded realm" and "the same whisper on an
//! unsharded one". Nothing pattern-matches them on the wire — the gateway answers every refusal with
//! `SMSG_CHAT_PLAYER_NOT_FOUND` carrying the name the player typed, on both planes — so these are
//! the LOG/`Err` surface, kept identical so a live tail reads the same either way.

/// No character anywhere in the realm answers to `name` (case-insensitively). The module's
/// `character_by_name` reads an IN-TRANSIT character as absent, and the gateway's realm-wide union
/// answers the same way for a name no connected shard holds — both land here.
pub fn no_player_named(name: &str) -> String {
    format!("no player named {name}")
}

/// The character exists but is not online. Vanilla has no offline whispering.
///
/// "Online" here is `game_character.online` — the SESSION flag, set by `player_login` — because that
/// is the flag the module's own `send_whisper` gate reads. It is deliberately NOT the group slice's
/// `game_world_entity` liveness test (`party::live_anywhere`): the two disagree for every playerbot,
/// and a whisper to a bot is refused today. See the gateway's `world::whisper` for the full note.
pub fn player_is_offline(name: &str) -> String {
    format!("{name} is offline")
}

/// The SENDER has no live entity — `send_whisper`'s `entity_by_owner` miss, and the gateway's
/// equivalent for a session with no in-world character (or one mid-shard-hop).
pub const NOT_IN_WORLD: &str = "whisperer not in world";

#[cfg(test)]
mod tests {
    use super::*;

    /// Pinned as RENDERED text: the point of the module is that both producers say the same thing,
    /// so the assertion is the string a live log (and a `/w` refusal path) actually shows.
    #[test]
    fn whisper_refusals_render_the_text_both_planes_share() {
        assert_eq!(no_player_named("Bob"), "no player named Bob");
        assert_eq!(player_is_offline("Bob"), "Bob is offline");
        assert_eq!(NOT_IN_WORLD, "whisperer not in world");
        // Case is passed through verbatim (the MATCH is case-insensitive; the message echoes what
        // the player typed, exactly as `SMSG_CHAT_PLAYER_NOT_FOUND` echoes the typed name).
        assert_eq!(no_player_named("bob"), "no player named bob");
    }
}
