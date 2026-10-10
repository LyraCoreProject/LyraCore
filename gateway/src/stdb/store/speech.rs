//! `Coordinator`'s [`SpeechStore`] adapter.

use anyhow::{anyhow, Result};

use super::chat::chat_outcome;
use crate::stdb::bindings::*;
use crate::stdb::connection::call_reducer;
use crate::stdb::Coordinator;
use crate::world::{Actor, ChatOutcome, SpeechStore};

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

impl Coordinator {
    /// Say, yell or `/e` on the speaker's Home Shard. The Module's language Gate answers a
    /// `chat:*` Refusal.
    pub fn send_chat(
        &self,
        _account_id: u64,
        actor_guid: u64,
        chat_type: u8,
        language: u8,
        message: String,
    ) -> Result<ChatOutcome> {
        let actor =
            Actor::new(actor_guid).ok_or_else(|| anyhow!("send_chat: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        chat_outcome(call_reducer!(
            coord.conn.reducers,
            "gw_send_chat",
            gw_send_chat_then(self.session_actor(actor), chat_type, language, message)
        ))
    }

    pub fn send_emote(
        &self,
        _account_id: u64,
        actor_guid: u64,
        text_emote: u32,
        emote_anim: u32,
        target_guid: u64,
    ) -> Result<()> {
        let actor =
            Actor::new(actor_guid).ok_or_else(|| anyhow!("send_emote: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_send_emote",
            gw_send_emote_then(
                self.session_actor(actor),
                text_emote,
                emote_anim,
                target_guid
            )
        )
    }

    /// GM playtest dot-command: resolve current Account authority on Realm-core, then convey it to
    /// the Home Shard through one Store operation.
    pub fn gm_command(&self, account_name: &str, actor_guid: u64, text: String) -> Result<()> {
        crate::realm_core::run_gm_command(self, account_name, actor_guid, text)
    }
}
