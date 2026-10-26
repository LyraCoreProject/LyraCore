use spacetimedb::{table, ReducerContext, Table};

use lyracore_shared::chat::chat_kind;
use lyracore_shared::constants::player_flags;

#[table(accessor = game_character_away)]
pub struct CharacterAway {
    #[primary_key]
    pub character_guid: u64,
    pub kind: u8,
    pub message: String,
}

pub(crate) const AFK_DEFAULT_REPLY: &str = "Away from Keyboard";
pub(crate) const DND_DEFAULT_REPLY: &str = "Do not Disturb";

const AWAY_BITS: u32 = player_flags::AFK | player_flags::DND;

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AwayChange {
    pub(crate) player_flags: u32,
    pub(crate) reply: Option<(u8, String)>,
}

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

pub(crate) fn apply_set_away(
    ctx: &ReducerContext,
    mut entity: crate::WorldEntity,
    kind: u8,
    message: String,
) -> Result<(), crate::Refusal> {
    if kind != chat_kind::AFK && kind != chat_kind::DND {
        return Err(format!("away kind {kind} is neither AFK nor DND").into());
    }
    let change = next_away(entity.player_flags, kind, &message);
    let character_guid = entity.guid;
    if change.player_flags != entity.player_flags {
        entity.player_flags = change.player_flags;
        entity.update(ctx);
    }
    write_reply(ctx, character_guid, change.reply);
    Ok(())
}

pub(crate) fn flags_at_entry(
    player_flags: u32,
    stored_kind: Option<u8>,
    entry: crate::world::WorldEntry,
) -> u32 {
    let flags = player_flags & !AWAY_BITS;
    match (entry, stored_kind) {
        (crate::world::WorldEntry::WorldPort, Some(kind)) => flags | flag_of(kind),
        _ => flags,
    }
}

pub(crate) fn at_world_entry(
    ctx: &ReducerContext,
    entity: &mut crate::WorldEntity,
    entry: crate::world::WorldEntry,
) {
    let rows = ctx.db.game_character_away();
    let stored_kind = rows.character_guid().find(entity.guid).map(|row| row.kind);
    entity.player_flags = flags_at_entry(entity.player_flags, stored_kind, entry);
    if entry == crate::world::WorldEntry::Login {
        rows.character_guid().delete(entity.guid);
    }
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
crate::character_owned!(transfer, fn sweep_transfer_game_character_away(ctx, character_guid, io) {
    table = game_character_away,
    primary_key = character_guid,
});

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

    #[test]
    fn afk_again_without_text_ends_it() {
        assert_eq!(next_away(0x02, AFK, ""), change(0x00, None));
        assert_eq!(next_away(0x04, DND, "   "), change(0x00, None));
    }

    #[test]
    fn afk_again_with_text_changes_only_the_reply() {
        assert_eq!(
            next_away(0x02, AFK, "Brb"),
            change(0x02, Some((AFK, "Brb")))
        );
    }

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
    fn a_login_ends_the_away_status_and_a_world_port_keeps_it() {
        use crate::world::WorldEntry::{Login, WorldPort};
        assert_eq!(flags_at_entry(0x00, Some(DND), Login), 0x00);
        assert_eq!(flags_at_entry(0x02, Some(AFK), Login), 0x00);
        assert_eq!(flags_at_entry(0x00, Some(DND), WorldPort), 0x04);
        assert_eq!(flags_at_entry(0x10, Some(AFK), WorldPort), 0x12);
        assert_eq!(flags_at_entry(0x10, None, WorldPort), 0x10);
        assert_eq!(flags_at_entry(0x06, None, WorldPort), 0x00);
    }

    #[test]
    fn a_reply_is_capped_at_255_characters() {
        let long = "a".repeat(300);
        let reply = next_away(0, AFK, &long).reply.unwrap().1;
        assert_eq!(reply.chars().count(), 255);
    }
}
