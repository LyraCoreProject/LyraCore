//! Away Status: AFK or DND on a live Character. Observers see the `PLAYER_FLAGS` bit on the live
//! entity through the entity Relay; [`CharacterAway`] holds the Auto-Reply a whisperer receives.
//! Both change together in one transaction. Login clears both, which covers Transfer arrival too.

use spacetimedb::{table, ReducerContext, Table};

use crate::game_world_entity;
use lyracore_shared::chat::chat_kind;
use lyracore_shared::constants::player_flags;

/// The Auto-Reply of one Character with an Away Status. Private: the owner-token Coordinator is
/// its only reader. [entity]
#[table(accessor = game_character_away)]
pub struct CharacterAway {
    #[primary_key]
    pub character_guid: u64,
    /// `chat_kind::AFK` or `chat_kind::DND`.
    pub kind: u8,
    pub message: String,
}

/// cm mangos.sql:3986 (`LANG_PLAYER_AFK_DEFAULT`), fx:GlobalStrings.lua:910-912.
pub(crate) const AFK_DEFAULT_REPLY: &str = "Away from Keyboard";
/// cm mangos.sql:3985 (`LANG_PLAYER_DND_DEFAULT`), fx:GlobalStrings.lua:910-912.
pub(crate) const DND_DEFAULT_REPLY: &str = "Do not Disturb";

const AWAY_BITS: u32 = player_flags::AFK | player_flags::DND;

/// The Auto-Reply a whisperer gets when the Character stored none.
pub(crate) fn default_reply(kind: u8) -> &'static str {
    if kind == chat_kind::DND {
        DND_DEFAULT_REPLY
    } else {
        AFK_DEFAULT_REPLY
    }
}

fn flag_of(kind: u8) -> u32 {
    if kind == chat_kind::DND {
        player_flags::DND
    } else {
        player_flags::AFK
    }
}

/// A Character's Away Status after one `/afk` or `/dnd`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AwayChange {
    /// The live entity's new `PLAYER_FLAGS`.
    pub(crate) player_flags: u32,
    /// The Auto-Reply to store, or `None` to delete it.
    pub(crate) reply: Option<(u8, String)>,
}

/// One `/afk` or `/dnd` against the current `PLAYER_FLAGS`, as cm:ChatHandler.cpp:641-693 applies
/// it. `kind` is `chat_kind::AFK` or `chat_kind::DND`.
///
/// - The kind is set and the text is empty: the kind ends.
/// - The kind is set and the text is not empty: only the Auto-Reply changes.
/// - The kind is not set: it starts with the text, or with the default text, and the other kind
///   ends.
///
/// There is no combat Gate: cmangos has none, and vmangos refuses AFK in combat
/// (vm:ChatHandler.cpp:613-614). LyraCore follows cmangos.
pub(crate) fn next_away(player_flags: u32, kind: u8, message: &str) -> AwayChange {
    let flag = flag_of(kind);
    let text = crate::chat::normalized_message(message);
    if player_flags & flag != 0 {
        return match text {
            None => AwayChange {
                player_flags: player_flags & !flag,
                reply: None,
            },
            Some(text) => AwayChange {
                player_flags,
                reply: Some((kind, text)),
            },
        };
    }
    let text = text.unwrap_or_else(|| default_reply(kind).to_string());
    AwayChange {
        player_flags: (player_flags & !AWAY_BITS) | flag,
        reply: Some((kind, text)),
    }
}

/// Apply one `/afk` or `/dnd` to `entity` and its Auto-Reply in this transaction.
pub(crate) fn apply_set_away(
    ctx: &ReducerContext,
    mut entity: crate::WorldEntity,
    kind: u8,
    message: String,
) -> Result<(), String> {
    if kind != chat_kind::AFK && kind != chat_kind::DND {
        return Err(format!("away kind {kind} is neither AFK nor DND"));
    }
    let change = next_away(entity.player_flags, kind, &message);
    let character_guid = entity.guid;
    if change.player_flags != entity.player_flags {
        entity.player_flags = change.player_flags;
        ctx.db.game_world_entity().guid().update(entity);
    }
    write_reply(ctx, character_guid, change.reply);
    Ok(())
}

/// End the Away Status of a Character entering the world. Called from login on the entity it is
/// about to insert, so Transfer arrival is covered too: nobody crosses a loading screen AFK.
pub(crate) fn end_at_login(ctx: &ReducerContext, entity: &mut crate::WorldEntity) {
    entity.player_flags &= !AWAY_BITS;
    ctx.db
        .game_character_away()
        .character_guid()
        .delete(entity.guid);
}

fn write_reply(ctx: &ReducerContext, character_guid: u64, reply: Option<(u8, String)>) {
    let rows = ctx.db.game_character_away();
    match reply {
        None => {
            rows.character_guid().delete(character_guid);
        }
        Some((kind, message)) => {
            let row = CharacterAway {
                character_guid,
                kind,
                message,
            };
            if rows.character_guid().find(character_guid).is_some() {
                rows.character_guid().update(row);
            } else {
                rows.insert(row);
            }
        }
    }
}

crate::character_owned!(delete, fn sweep_delete_game_character_away(ctx, character_guid) {
    ctx.db.game_character_away().character_guid().delete(character_guid);
});
crate::character_owned!(not_transported, fn sweep_transfer_game_character_away());

#[cfg(test)]
mod tests {
    use super::*;

    const AFK: u8 = 0x14;
    const DND: u8 = 0x15;
    const GHOST: u32 = 0x10;

    fn change(player_flags: u32, reply: Option<(u8, &str)>) -> AwayChange {
        AwayChange {
            player_flags,
            reply: reply.map(|(kind, text)| (kind, text.to_string())),
        }
    }

    /// cm:ChatHandler.cpp:669-682: `/afk` with no text sets the flag and the default text.
    #[test]
    fn afk_without_text_starts_with_the_default_reply() {
        assert_eq!(
            next_away(0, AFK, ""),
            change(0x02, Some((AFK, "Away from Keyboard")))
        );
        assert_eq!(
            next_away(0, DND, ""),
            change(0x04, Some((DND, "Do not Disturb")))
        );
    }

    #[test]
    fn afk_with_text_stores_the_trimmed_text() {
        assert_eq!(
            next_away(0, AFK, "  brb food "),
            change(0x02, Some((AFK, "brb food")))
        );
    }

    /// cm:ChatHandler.cpp:649-656: the kind is set and the text is empty, so it ends.
    #[test]
    fn afk_again_without_text_ends_it() {
        assert_eq!(next_away(0x02, AFK, ""), change(0x00, None));
        assert_eq!(next_away(0x04, DND, "   "), change(0x00, None));
    }

    /// cm:ChatHandler.cpp:658-666: the kind is set and there is text, so only the text changes.
    #[test]
    fn afk_again_with_text_changes_only_the_reply() {
        assert_eq!(
            next_away(0x02, AFK, "Brb"),
            change(0x02, Some((AFK, "Brb")))
        );
    }

    /// cm:ChatHandler.cpp:677-690: setting one kind ends the other.
    #[test]
    fn dnd_while_afk_switches_and_the_reverse() {
        assert_eq!(
            next_away(0x02, DND, ""),
            change(0x04, Some((DND, "Do not Disturb")))
        );
        assert_eq!(
            next_away(0x04, AFK, "lunch"),
            change(0x02, Some((AFK, "lunch")))
        );
    }

    #[test]
    fn other_player_flags_survive_every_change() {
        assert_eq!(
            next_away(GHOST, AFK, ""),
            change(0x12, Some((AFK, "Away from Keyboard")))
        );
        assert_eq!(next_away(0x12, AFK, ""), change(0x10, None));
    }

    #[test]
    fn a_reply_is_capped_at_255_characters() {
        let long = "a".repeat(300);
        let reply = next_away(0, AFK, &long).reply.unwrap().1;
        assert_eq!(reply.chars().count(), 255);
    }
}
