use spacetimedb::{table, Identity, ReducerContext, Table, Timestamp};

use crate::game_world_entity;
use lyracore_shared::social::ContactRefusal;

pub const CHAT_SAY: u8 = lyracore_shared::chat::broadcast_chat::SAY;
pub const CHAT_YELL: u8 = lyracore_shared::chat::broadcast_chat::YELL;
pub const CHAT_TEXT_EMOTE: u8 = lyracore_shared::chat::broadcast_chat::CREATURE_TEXT_EMOTE;
pub const CHAT_EMOTE: u8 = lyracore_shared::chat::broadcast_chat::EMOTE;

const MAX_CHAT_LEN: usize = 255;

#[table(accessor = game_chat_event, public)]
#[table(accessor = game_chat_event_live, public, event)]
pub struct ChatEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub sender_guid: u64,
    pub chat_type: u8,
    pub language: u8,
    pub message: String,
    pub created_at: Timestamp,
    #[default(0u64)]
    pub target_guid: u64,
}

pub(crate) fn is_supported_chat_type(chat_type: u8) -> bool {
    use lyracore_shared::chat::broadcast_chat::{
        CREATURE_BOSS_EMOTE, CREATURE_WHISPER, CREATURE_ZONE_EMOTE, CREATURE_ZONE_YELL,
    };
    matches!(
        chat_type,
        CHAT_SAY
            | CHAT_YELL
            | CHAT_TEXT_EMOTE
            | CREATURE_BOSS_EMOTE
            | CREATURE_WHISPER
            | CREATURE_ZONE_YELL
            | CREATURE_ZONE_EMOTE
    )
}

pub(crate) fn normalized_message(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(MAX_CHAT_LEN).collect())
}

// Addons frame their own bytes; trimming would alter the payload in transit.
pub(crate) fn addon_payload(raw: &str) -> Option<String> {
    (!raw.is_empty()).then(|| raw.chars().take(MAX_CHAT_LEN).collect())
}

pub(crate) fn apply_send_chat(
    ctx: &ReducerContext,
    sender: crate::WorldEntity,
    chat_type: u8,
    language: u8,
    message: String,
) -> Result<(), String> {
    admit_player_line(chat_type, sender.race(), language)?;
    write_chat_event(ctx, sender, 0, chat_type, language, message)
}

fn admit_player_line(chat_type: u8, race: u8, language: u8) -> Result<(), String> {
    use lyracore_shared::chat::chat_kind;
    let kind = match chat_type {
        CHAT_SAY => chat_kind::SAY,
        CHAT_YELL => chat_kind::YELL,
        CHAT_EMOTE => chat_kind::EMOTE,
        _ => return Err(format!("unsupported chat type {chat_type}")),
    };
    lyracore_shared::chat::speakable_language(kind, race, u32::from(language))
        .map(drop)
        .map_err(|refusal| refusal.as_tag().to_string())
}

pub(crate) fn apply_send_chat_to(
    ctx: &ReducerContext,
    sender: crate::WorldEntity,
    target_guid: u64,
    chat_type: u8,
    language: u8,
    message: String,
) -> Result<(), String> {
    if !is_supported_chat_type(chat_type) {
        return Err(format!("unsupported chat type {chat_type}"));
    }
    if chat_type == lyracore_shared::chat::broadcast_chat::CREATURE_WHISPER
        && !ctx
            .db
            .game_world_entity()
            .guid()
            .find(target_guid)
            .is_some_and(|target| target.is_player())
    {
        return Err("creature whisper requires a Character recipient".to_string());
    }
    write_chat_event(ctx, sender, target_guid, chat_type, language, message)
}

fn write_chat_event(
    ctx: &ReducerContext,
    mut sender: crate::WorldEntity,
    target_guid: u64,
    chat_type: u8,
    language: u8,
    message: String,
) -> Result<(), String> {
    if sender.dead {
        return Err("dead players cannot speak".to_string());
    }
    let text = normalized_message(&message).ok_or_else(|| "empty message".to_string())?;
    let language = if chat_type == CHAT_EMOTE {
        lyracore_shared::chat::language::UNIVERSAL as u8
    } else {
        language
    };
    if lyracore_shared::chat::broadcast_chat::is_zone_wide(chat_type) {
        sender.zone_id =
            crate::terrain::zone_id_at(ctx, sender.map_id, sender.x, sender.y).unwrap_or(0);
        sender = sender.update(ctx);
    }
    let event = ctx.db.game_chat_event_live().insert(ChatEvent {
        id: 0,
        sender_guid: sender.guid,
        chat_type,
        language,
        message: text,
        created_at: ctx.timestamp,
        target_guid,
    });
    crate::native::speech(ctx, &sender, &event);
    Ok(())
}

#[table(accessor = game_channel_member, public, index(accessor = by_channel, btree(columns = [channel])))]
pub struct ChannelMember {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub channel: String,
    pub character_guid: u64,
    pub owner_identity: Identity,
}

#[table(accessor = game_channel_event, public)]
pub struct ChannelEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub channel: String,
    pub channel_display: String,
    pub sender_guid: u64,
    pub message: String,
    pub created_at: Timestamp,
}

#[table(
    accessor = game_emote_event,
    public,
    index(accessor = by_grid, btree(columns = [map_id, instance_id, grid_x, grid_y]))
)]
#[table(accessor = game_emote_event_live, public, event)]
pub struct EmoteEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub sender_guid: u64,
    pub text_emote: u32,
    pub emote_anim: u32,
    pub created_at: Timestamp,
    #[default(0u64)]
    pub target_guid: u64,
    #[default(0u32)]
    pub map_id: u32,
    #[default(0u64)]
    pub instance_id: u64,
    #[default(0i32)]
    pub grid_x: i32,
    #[default(0i32)]
    pub grid_y: i32,
}

pub(crate) fn apply_send_emote(
    ctx: &ReducerContext,
    sender: crate::WorldEntity,
    text_emote: u32,
    emote_anim: u32,
    target_guid: u64,
) -> Result<(), crate::Refusal> {
    if sender.dead {
        return Err("dead players cannot emote".into());
    }
    let (map_id, instance_id, grid_x, grid_y) = crate::helpers::entity_addr(&sender);
    let event = ctx.db.game_emote_event_live().insert(EmoteEvent {
        id: 0,
        sender_guid: sender.guid,
        text_emote,
        emote_anim,
        created_at: ctx.timestamp,
        target_guid,
        map_id,
        instance_id,
        grid_x,
        grid_y,
    });
    crate::native::text_emote(ctx, &sender, &event);
    if sender.is_player() && target_guid != 0 {
        crate::creatures::eventai_on_receive_emote(ctx, target_guid, sender.guid, text_emote);
    }
    Ok(())
}

#[table(accessor = game_whisper_event, public, index(accessor = by_recipient, btree(columns = [recipient_identity])))]
pub struct WhisperEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub recipient_identity: Identity,
    pub other_guid: u64,
    pub is_inform: bool,
    pub message: String,
    pub created_at: Timestamp,
    #[default(0u64)]
    pub recipient_guid: u64,
}

#[table(accessor = game_system_message_event, public, index(accessor = by_recipient, btree(columns = [recipient_identity])))]
pub struct SystemMessageEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub recipient_identity: Identity,
    pub recipient_guid: u64,
    pub message: String,
    pub created_at: Timestamp,
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn emit_system_message(
    ctx: &ReducerContext,
    character_guid: u64,
    message: String,
) -> Result<(), String> {
    let recipient = crate::helpers::character_by_guid(ctx, character_guid)
        .map(|character| (character.owner_identity, character.online));
    let event = prepare_system_message(character_guid, recipient, &message, ctx.timestamp)?;
    let event = ctx.db.game_system_message_event().insert(event);
    crate::native::system_message(ctx, &event);
    Ok(())
}

fn prepare_system_message(
    character_guid: u64,
    recipient: Option<(Identity, bool)>,
    message: &str,
    created_at: Timestamp,
) -> Result<SystemMessageEvent, String> {
    let Some((recipient_identity, online)) = recipient else {
        return Err(format!(
            "system message recipient {character_guid} is not present on this Shard"
        ));
    };
    if !online {
        return Err(format!(
            "system message recipient {character_guid} is offline"
        ));
    }
    let message = normalized_message(message).ok_or_else(|| "empty system message".to_string())?;
    Ok(SystemMessageEvent {
        id: 0,
        recipient_identity,
        recipient_guid: character_guid,
        message,
        created_at,
    })
}

pub const MAX_FRIENDS: usize = 50;
pub const MAX_IGNORED: usize = 25;

#[table(accessor = game_character_contact, public,
    index(accessor = by_owner, btree(columns = [owner_guid])),
    index(accessor = by_target, btree(columns = [target_guid]))
)]
pub struct ContactEntry {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub owner_guid: u64,
    pub owner_identity: Identity,
    pub target_guid: u64,
    pub is_ignore: bool,
}

crate::character_owned!(delete, fn sweep_delete_game_character_contact(ctx, character_guid) {
    let contacts = ctx.db.game_character_contact();
    for r in contacts.by_owner().filter(&character_guid).collect::<Vec<_>>() {
        contacts.id().delete(r.id);
    }
    for r in contacts.by_target().filter(character_guid).collect::<Vec<_>>() {
        contacts.id().delete(r.id);
    }
});
crate::character_owned!(transfer, fn sweep_transfer_game_character_contact(ctx, character_guid, io) {
    table = game_character_contact,
    by = by_owner,
    remint = id,
});
crate::character_owned!(restamp, fn sweep_restamp_game_character_contact(ctx, character_guid, identity) {
    let contacts = ctx.db.game_character_contact();
    for mut r in contacts.by_owner().filter(&character_guid).collect::<Vec<_>>() {
        if r.owner_identity != identity {
            r.owner_identity = identity;
            contacts.id().update(r);
        }
    }
});

pub(crate) fn refused_contact(refusal: ContactRefusal, detail: &str) -> String {
    let tag = refusal.as_tag();
    spacetimedb::log::info!("contact refused {tag}: {detail}");
    tag.to_string()
}

pub(crate) fn add_contact(
    ctx: &ReducerContext,
    sender: crate::WorldEntity,
    target_guid: u64,
    is_ignore: bool,
    target_race: Option<u8>,
) -> Result<(), String> {
    let owner_guid = sender.guid;
    add_contact_core(ctx, sender, target_guid, is_ignore, target_race).map_err(|refusal| {
        refused_contact(
            refusal,
            &format!("{owner_guid} could not add {target_guid}"),
        )
    })
}

fn add_contact_core(
    ctx: &ReducerContext,
    sender: crate::WorldEntity,
    target_guid: u64,
    is_ignore: bool,
    target_race: Option<u8>,
) -> Result<(), ContactRefusal> {
    if target_guid == sender.guid {
        return Err(ContactRefusal::AddSelf);
    }
    let refuses_enemy = !is_ignore
        && target_race
            .is_some_and(|race| !lyracore_shared::faction::same_team(sender.race(), race));
    if refuses_enemy {
        return Err(ContactRefusal::Enemy);
    }
    let contacts = ctx.db.game_character_contact();
    let existing: Vec<_> = contacts.by_owner().filter(&sender.guid).collect();
    if existing
        .iter()
        .any(|c| c.target_guid == target_guid && c.is_ignore == is_ignore)
    {
        return Err(ContactRefusal::AlreadyOnList);
    }
    let cap = if is_ignore { MAX_IGNORED } else { MAX_FRIENDS };
    if existing.iter().filter(|c| c.is_ignore == is_ignore).count() >= cap {
        return Err(ContactRefusal::ListFull);
    }
    contacts.insert(ContactEntry {
        id: 0,
        owner_guid: sender.guid,
        owner_identity: sender.owner_identity,
        target_guid,
        is_ignore,
    });
    Ok(())
}

pub(crate) fn remove_contact(
    ctx: &ReducerContext,
    sender: crate::WorldEntity,
    target_guid: u64,
    is_ignore: bool,
) -> Result<(), String> {
    let contacts = ctx.db.game_character_contact();
    let Some(row) = contacts
        .by_owner()
        .filter(&sender.guid)
        .find(|c| c.target_guid == target_guid && c.is_ignore == is_ignore)
    else {
        return Err(refused_contact(
            ContactRefusal::NotOnList,
            &format!("{} does not hold {target_guid}", sender.guid),
        ));
    };
    contacts.id().delete(row.id);
    Ok(())
}

#[table(
    accessor = game_roll_event,
    public,
    index(accessor = by_grid, btree(columns = [map_id, instance_id, grid_x, grid_y]))
)]
pub struct RollEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub roller_guid: u64,
    pub min_roll: u32,
    pub max_roll: u32,
    pub result: u32,
    pub created_at: Timestamp,
    #[default(0u32)]
    pub map_id: u32,
    #[default(0u64)]
    pub instance_id: u64,
    #[default(0i32)]
    pub grid_x: i32,
    #[default(0i32)]
    pub grid_y: i32,
}

pub(crate) fn normalized_roll_range(min_roll: u32, max_roll: u32) -> (u32, u32) {
    const ROLL_MAX: u32 = 10_000;
    let lo = min_roll.min(max_roll).min(ROLL_MAX);
    let hi = min_roll.max(max_roll).min(ROLL_MAX);
    (lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_named_creature_broadcast_types_are_supported() {
        assert!(is_supported_chat_type(CHAT_SAY));
        assert!(is_supported_chat_type(CHAT_YELL));
        assert!(is_supported_chat_type(CHAT_TEXT_EMOTE));
        assert!(!is_supported_chat_type(CHAT_EMOTE));
        assert!(!is_supported_chat_type(255));
    }

    #[test]
    fn a_player_line_needs_a_language_the_speakers_race_knows() {
        const HUMAN: u8 = 1;
        const ORC: u8 = 2;
        for chat_type in [CHAT_SAY, CHAT_YELL, CHAT_EMOTE] {
            assert_eq!(
                admit_player_line(chat_type, HUMAN, 1),
                Err("chat:unknown_language".to_string()),
                "a Human in Orcish, chat type {chat_type}"
            );
            assert_eq!(admit_player_line(chat_type, HUMAN, 7), Ok(()));
            assert_eq!(admit_player_line(chat_type, ORC, 1), Ok(()));
            assert_eq!(admit_player_line(chat_type, HUMAN, 0), Ok(()));
        }
    }

    #[test]
    fn a_player_cannot_submit_the_creature_text_emote() {
        assert_eq!(
            admit_player_line(CHAT_TEXT_EMOTE, 1, 0),
            Err("unsupported chat type 2".to_string())
        );
    }

    #[test]
    fn an_addon_payload_keeps_its_whitespace() {
        assert_eq!(
            addon_payload("LCTEST\t ping ").as_deref(),
            Some("LCTEST\t ping ")
        );
        assert_eq!(addon_payload(" "), Some(" ".to_string()));
        assert_eq!(addon_payload(""), None);
        assert_eq!(
            addon_payload(&"a".repeat(1000)).unwrap().chars().count(),
            MAX_CHAT_LEN
        );
    }

    #[test]
    fn message_is_trimmed_capped_and_empty_rejected() {
        assert_eq!(normalized_message("  hello  ").as_deref(), Some("hello"));
        assert_eq!(normalized_message(""), None);
        assert_eq!(normalized_message("   \t  "), None);
        let long: String = "a".repeat(1000);
        assert_eq!(
            normalized_message(&long).unwrap().chars().count(),
            MAX_CHAT_LEN
        );
        let unicode = "🦀".repeat(300);
        assert_eq!(
            normalized_message(&unicode).as_deref(),
            Some("🦀".repeat(MAX_CHAT_LEN).as_str())
        );
    }

    #[test]
    fn an_online_recipient_produces_one_bounded_system_message_event() {
        let recipient_identity = Identity::from_byte_array([7; 32]);
        let created_at = Timestamp::from_micros_since_unix_epoch(42);
        let event = prepare_system_message(
            9001,
            Some((recipient_identity, true)),
            &format!("  {}  ", "🦀".repeat(300)),
            created_at,
        )
        .expect("an online Character accepts one System Message");

        assert_eq!(event.id, 0);
        assert_eq!(event.recipient_identity, recipient_identity);
        assert_eq!(event.recipient_guid, 9001);
        assert_eq!(event.message, "🦀".repeat(MAX_CHAT_LEN));
        assert_eq!(event.created_at, created_at);
    }

    #[test]
    fn a_missing_or_offline_recipient_produces_no_system_message_event() {
        let now = Timestamp::UNIX_EPOCH;
        let identity = Identity::from_byte_array([3; 32]);

        assert_eq!(
            prepare_system_message(9001, None, "hello", now)
                .err()
                .as_deref(),
            Some("system message recipient 9001 is not present on this Shard")
        );
        assert_eq!(
            prepare_system_message(9001, Some((identity, false)), "hello", now)
                .err()
                .as_deref(),
            Some("system message recipient 9001 is offline")
        );
    }

    #[test]
    fn an_empty_system_message_produces_no_event() {
        let identity = Identity::from_byte_array([3; 32]);

        assert_eq!(
            prepare_system_message(9001, Some((identity, true)), "  \t ", Timestamp::UNIX_EPOCH)
                .err()
                .as_deref(),
            Some("empty system message")
        );
    }

    #[test]
    fn roll_range_swaps_an_inverted_pair_and_caps_both_ends_at_10000() {
        assert_eq!(normalized_roll_range(1, 100), (1, 100));
        assert_eq!(normalized_roll_range(100, 1), (1, 100));
        assert_eq!(normalized_roll_range(5, 5), (5, 5));
        assert_eq!(normalized_roll_range(0, 0), (0, 0));
        assert_eq!(normalized_roll_range(1, 2_000_000), (1, 10_000));
        assert_eq!(
            normalized_roll_range(3_000_000, 2_000_000),
            (10_000, 10_000)
        );
        assert_eq!(normalized_roll_range(1, 10_000), (1, 10_000));
    }
}
