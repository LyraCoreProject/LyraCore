//! `Coordinator`'s [`ChatActionStore`] adapter.

use anyhow::Result;
use lyracore_shared::chat::ChatRefusal;

use crate::stdb::bindings::*;
use crate::stdb::connection::{call_reducer, reducer_refusal_reason};
use crate::stdb::Coordinator;
use crate::world::{whisper, Actor, ChatActionStore, ChatOutcome};

impl ChatActionStore for Coordinator {
    /// The Speaker Facts for `speaker_guid` on this Home Shard: race from `UNIT_FIELD_BYTES_0`
    /// byte 0 and the chat tag from `PLAYER_FLAGS`, both off the live entity, plus the Character's
    /// name. `None` when the speaker has no live entity here.
    fn speaker_facts(&self, speaker_guid: u64) -> Result<Option<crate::world::SpeakerFacts>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let Some(entity) = db.game_world_entity().guid().find(&speaker_guid) else {
            return Ok(None);
        };
        let name = db
            .game_character()
            .guid()
            .find(&speaker_guid)
            .map(|character| character.name)
            .unwrap_or_default();
        Ok(Some(crate::world::SpeakerFacts {
            race: (entity.unit_bytes_0 & 0xFF) as u8,
            chat_tag: lyracore_shared::chat::chat_tag_for(entity.player_flags),
            name,
        }))
    }

    /// `realm_chat`: commit one Realm Chat Line on Realm-core, or on the one database of an
    /// unsharded Realm. The Module applies every chat Gate. The speaker's name stays here.
    fn realm_chat(
        &self,
        actor: Actor,
        request: crate::world::RealmChatRequest,
    ) -> Result<ChatOutcome> {
        let realm = self.realm_core()?;
        let request = RealmChatRequest {
            kind: request.kind,
            language: request.language,
            channel_name: request.channel_name,
            target_guid: request.target_guid,
            message: request.message,
            speaker: SpeakerFacts {
                race: request.speaker.race,
                chat_tag: request.speaker.chat_tag,
            },
        };
        chat_outcome(call_reducer!(
            realm.0.call_pipe().conn.reducers,
            "realm_chat",
            realm_chat_then(realm.session_actor(actor), request)
        ))
    }

    /// `gw_set_away`: one `/afk` or `/dnd` on the Character's Home Shard, where its live entity
    /// and Auto-Reply live.
    fn set_away(&self, actor: Actor, kind: u8, message: String) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_set_away",
            gw_set_away_then(self.session_actor(actor), kind, message)
        )
    }

    fn whisper_target(
        &self,
        speaker_guid: u64,
        typed_name: &str,
    ) -> Result<Option<crate::world::WhisperTargetFacts>> {
        whisper::target_facts(self, speaker_guid, typed_name)
    }

    /// `realm_whisper`: commit one whisper's lines on Realm-core, or on the one database of an
    /// unsharded Realm. The Module applies every whisper Gate. The speaker's name stays here.
    fn realm_whisper(
        &self,
        actor: Actor,
        request: crate::world::WhisperRequest,
    ) -> Result<ChatOutcome> {
        let realm = self.realm_core()?;
        let target = request.target;
        let request = WhisperRequest {
            language: request.language,
            message: request.message,
            speaker: SpeakerFacts {
                race: request.speaker.race,
                chat_tag: request.speaker.chat_tag,
            },
            target: WhisperTargetFacts {
                guid: target.guid,
                race: target.race,
                name: target.name,
                ignores_speaker: target.ignores_speaker,
                away_kind: target.away_kind,
                away_message: target.away_message,
            },
        };
        chat_outcome(call_reducer!(
            realm.0.call_pipe().conn.reducers,
            "realm_whisper",
            realm_whisper_then(realm.session_actor(actor), request)
        ))
    }

    fn speaker_gm_level(&self, speaker_guid: u64) -> Result<u8> {
        Ok(self.home_gm_level(speaker_guid))
    }
}

/// The Module's typed chat Refusal. Only a reducer the Module rejected carries a tag; a timeout,
/// transport, or SDK failure stays an error with an unknown outcome.
pub(super) fn chat_outcome(result: Result<()>) -> Result<ChatOutcome> {
    match result {
        Ok(()) => Ok(ChatOutcome::Delivered),
        Err(error) => match reducer_refusal_reason(&error).and_then(ChatRefusal::parse_tag) {
            Some(refusal) => Ok(ChatOutcome::Refused(refusal)),
            None => Err(error),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stdb::{classify, DurableFailure, ReducerCallError};

    #[test]
    fn a_rejected_chat_tag_decodes_to_its_refusal() {
        let error = ReducerCallError::refused("realm_chat", ChatRefusal::NotInGroup.as_tag());
        assert_eq!(
            chat_outcome(Err(error.into())).unwrap(),
            ChatOutcome::Refused(ChatRefusal::NotInGroup)
        );
    }

    #[test]
    fn a_delivered_line_decodes_to_delivered() {
        assert_eq!(chat_outcome(Ok(())).unwrap(), ChatOutcome::Delivered);
    }

    #[test]
    fn an_unknown_rejection_stays_a_refusal_for_the_handler() {
        let error = chat_outcome(Err(
            ReducerCallError::refused("realm_chat", "mystery").into()
        ))
        .unwrap_err();
        assert!(matches!(classify(&error), DurableFailure::Refusal { .. }));
    }

    #[test]
    fn a_lost_transport_stays_a_transport_loss() {
        let error =
            chat_outcome(Err(ReducerCallError::transport_lost("realm_chat").into())).unwrap_err();
        assert_eq!(classify(&error), DurableFailure::TransportLoss);
    }
}
