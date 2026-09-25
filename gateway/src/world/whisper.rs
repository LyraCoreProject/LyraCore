//! Whisper target facts: the realm-wide reads a whisper needs before the Module can decide it.
//!
//! A whisper is a set of Realm Chat Lines (`handlers::chat`). The Module applies every whisper
//! rule on Realm-core, which holds no Characters, contact rows or live entities. So the Gateway
//! reads the target on whichever World Shard holds it and conveys the facts in the Durable
//! Request. Sharded and unsharded Gateways run this same path; on an unsharded Gateway
//! `world_stores()` is empty and every read is one cache read.

use anyhow::Result;

use super::presence::{self, AwayStatus};
use super::{WhisperTargetFacts, WorldStore};
use lyracore_shared::chat::chat_kind;

/// The online Character `typed_name` reaches, with the facts the Module's whisper Gates need.
/// `None` when no online Character carries that name.
///
/// Names are unique per Shard, not per Realm, so the name can resolve to several Characters. The
/// target is the first one whose `session_online` is set: `/w` addresses a logged-in Character.
/// The session flag, not a live entity, so a bot stays unwhisperable (bots never log in) and a
/// Character on a loading screen or in Transfer stays whisperable.
///
/// A candidate whose Realm Presence cannot be read is skipped; if no other candidate is online,
/// that read's error is the answer, so the caller never reports a Character missing when a Shard
/// only failed to answer.
pub(crate) fn target_facts<St: WorldStore + ?Sized>(
    store: &St,
    speaker_guid: u64,
    typed_name: &str,
) -> Result<Option<WhisperTargetFacts>> {
    let mut unreadable = None;
    let mut target = None;
    for guid in presence::resolve_all_by_name(store, typed_name)? {
        match presence::of(store, guid) {
            Ok(Some(presence)) if presence.session_online => {
                target = Some(presence);
                break;
            }
            Ok(_) => {}
            Err(error) => unreadable = Some(error),
        }
    }
    let Some(target) = target else {
        return match unreadable {
            Some(error) => Err(error),
            None => Ok(None),
        };
    };
    let ignores_speaker = ignored_anywhere(store, target.guid, speaker_guid)?;
    let (away_kind, away_message) = match presence::auto_reply(store, &target)? {
        Some(reply) => (away_kind_of(reply.away), reply.message),
        None => (0, String::new()),
    };
    Ok(Some(WhisperTargetFacts {
        guid: target.guid,
        race: target.race,
        name: target.name,
        ignores_speaker,
        away_kind,
        away_message,
    }))
}

fn away_kind_of(away: AwayStatus) -> u8 {
    match away {
        AwayStatus::None => 0,
        AwayStatus::Afk => chat_kind::AFK,
        AwayStatus::Dnd => chat_kind::DND,
    }
}

/// Does `owner_guid` have `other_guid` on their ignore list, on whichever Shard holds their
/// contact rows?
///
/// `game_character_contact` is character-owned and travels with the Character, so exactly one
/// connected database has the rows, and which one is not knowable from here. The union is the
/// answer, in the same shape [`presence::resolve_by_name`] uses. `world_stores()` is empty on a
/// single-database Gateway, so this is one cache read there.
///
/// A peer Shard that cannot answer contributes `false`, never `true`. Reading an `Err` as
/// "ignored" would drop the line and tell every sender "X is ignoring you" for as long as one peer
/// Shard is down. The other direction costs one whisper that reaches someone who ignores the
/// sender, and that client still drops it and answers `CMSG_CHAT_IGNORED`.
///
/// The session's own handle propagates its `Err` instead: the whole session already reads through
/// that database, so a failure there is not "one Shard is down".
pub(crate) fn ignored_anywhere<St: WorldStore + ?Sized>(
    store: &St,
    owner_guid: u64,
    other_guid: u64,
) -> Result<bool> {
    if store.ignored_guids(owner_guid)?.contains(&other_guid) {
        return Ok(true);
    }
    Ok(store.world_stores().iter().any(|shard| {
        shard
            .ignored_guids(owner_guid)
            .map(|ignored| ignored.contains(&other_guid))
            .unwrap_or(false)
    }))
}
