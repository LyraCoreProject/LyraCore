//! `Coordinator`'s [`SpeechStore`] adapter.

use anyhow::Result;

use crate::world::{ChatOutcome, SpeechStore};

use crate::stdb::Coordinator;

impl SpeechStore for Coordinator {
    fn send_chat(
        &self,
        account_id: u64,
        self_guid: u64,
        chat_type: u8,
        language: u8,
        message: String,
    ) -> Result<ChatOutcome> {
        self.send_chat(account_id, self_guid, chat_type, language, message)
    }

    fn send_emote(
        &self,
        account_id: u64,
        self_guid: u64,
        text_emote: u32,
        emote_anim: u32,
        target_guid: u64,
    ) -> Result<()> {
        self.send_emote(account_id, self_guid, text_emote, emote_anim, target_guid)
    }

    fn gm_command(&self, account_name: &str, self_guid: u64, text: String) -> Result<()> {
        self.gm_command(account_name, self_guid, text)
    }
}
