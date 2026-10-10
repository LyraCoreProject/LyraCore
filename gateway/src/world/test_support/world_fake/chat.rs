use super::super::*;

#[derive(Default)]
pub(crate) struct ChatState {
    /// What `speaker_facts` answers for every speaker. `None` models a speaker with no live entity.
    pub(crate) speaker_facts: Option<SpeakerFacts>,
    /// Every Character's GM level, as `speaker_gm_level` reads it.
    pub(crate) gm_level: u8,
    /// Recorded `realm_chat` requests, with the speaker guid the session authenticated.
    pub(crate) realm_chats: std::sync::Mutex<Vec<(u64, RealmChatRequest)>>,
    /// Recorded `realm_whisper` requests, with the speaker guid the session authenticated.
    pub(crate) realm_whispers: std::sync::Mutex<Vec<(u64, WhisperRequest)>>,
}

impl ChatActionStore for WorldFake {
    fn speaker_facts(&self, _speaker_guid: u64) -> Result<Option<SpeakerFacts>> {
        Ok(self.chat.speaker_facts.clone())
    }

    fn realm_chat(&self, actor: Actor, request: RealmChatRequest) -> Result<ChatOutcome> {
        self.chat
            .realm_chats
            .lock()
            .unwrap()
            .push((actor.guid(), request));
        Ok(ChatOutcome::Delivered)
    }

    fn set_away(&self, _actor: Actor, _kind: u8, _message: String) -> Result<()> {
        Ok(())
    }

    fn whisper_target(
        &self,
        speaker_guid: u64,
        typed_name: &str,
    ) -> Result<Option<WhisperTargetFacts>> {
        whisper::target_facts(self, speaker_guid, typed_name)
    }

    /// The Module's `realm_whisper`, modelled: it records what the Gateway conveyed before it
    /// answers, because the speaker guid is the whole authorization of the call.
    fn realm_whisper(&self, actor: Actor, request: WhisperRequest) -> Result<ChatOutcome> {
        self.rec("realm_whisper");
        self.chat
            .realm_whispers
            .lock()
            .unwrap()
            .push((actor.guid(), request));
        Ok(ChatOutcome::Delivered)
    }

    fn speaker_gm_level(&self, _speaker_guid: u64) -> Result<u8> {
        Ok(self.chat.gm_level)
    }
}
