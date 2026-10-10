//! `Coordinator`'s [`ChatActionStore`] adapter.

use anyhow::Result;
use lyracore_shared::chat::ChatRefusal;

use crate::stdb::bindings::*;
use crate::stdb::connection::{call_reducer, reducer_refusal_reason};
use crate::stdb::Coordinator;
use crate::world::{whisper, ChatActionStore, ChatOutcome};

impl ChatActionStore for crate::stdb::Coordinator {
    fn speaker_facts(&self, speaker_guid: u64) -> Result<Option<crate::world::SpeakerFacts>> {
        crate::stdb::Coordinator::speaker_facts(self, speaker_guid)
    }

    fn realm_chat(
        &self,
        speaker_guid: u64,
        request: crate::world::RealmChatRequest,
    ) -> Result<ChatOutcome> {
        crate::stdb::Coordinator::realm_chat(self, speaker_guid, request)
    }

    fn set_away(&self, speaker_guid: u64, kind: u8, message: String) -> Result<()> {
        crate::stdb::Coordinator::set_away(self, speaker_guid, kind, message)
    }

    fn whisper_target(
        &self,
        speaker_guid: u64,
        typed_name: &str,
    ) -> Result<Option<crate::world::WhisperTargetFacts>> {
        whisper::target_facts(self, speaker_guid, typed_name)
    }

    fn realm_whisper(
        &self,
        speaker_guid: u64,
        request: crate::world::WhisperRequest,
    ) -> Result<ChatOutcome> {
        crate::stdb::Coordinator::realm_whisper(self, speaker_guid, request)
    }

    fn speaker_gm_level(&self, speaker_guid: u64) -> Result<u8> {
        Ok(crate::stdb::Coordinator::home_gm_level(self, speaker_guid))
    }
}

impl Coordinator {
    /// The Speaker Facts for `speaker_guid` on this Home Shard: race from `UNIT_FIELD_BYTES_0`
    /// byte 0 and the chat tag from `PLAYER_FLAGS`, both off the live entity, plus the Character's
    /// name. `None` when the speaker has no live entity here.
    pub(crate) fn speaker_facts(
        &self,
        speaker_guid: u64,
    ) -> anyhow::Result<Option<crate::world::SpeakerFacts>> {
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

    /// `gw_set_away`: one `/afk` or `/dnd` on the Character's Home Shard, where its live entity
    /// and Auto-Reply live.
    pub fn set_away(&self, actor_guid: u64, kind: u8, message: String) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_set_away",
            gw_set_away_then(self.actor_or_owner(actor_guid), kind, message)
        )
    }

    /// `realm_chat`: commit one Realm Chat Line on Realm-core, or on the one database of an
    /// unsharded Realm. The Module applies every chat Gate. The speaker's name stays here.
    pub fn realm_chat(
        &self,
        speaker_guid: u64,
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
            realm_chat_then(realm.actor_or_owner(speaker_guid), request)
        ))
    }

    /// `realm_whisper`: commit one whisper's lines on Realm-core, or on the one database of an
    /// unsharded Realm. The Module applies every whisper Gate. The speaker's name stays here.
    pub fn realm_whisper(
        &self,
        speaker_guid: u64,
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
            realm_whisper_then(realm.actor_or_owner(speaker_guid), request)
        ))
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
