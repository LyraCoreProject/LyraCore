//! `Coordinator`'s [`SpeechStore`] adapter.

use anyhow::Result;

use super::chat::chat_outcome;
use crate::stdb::bindings::*;
use crate::stdb::connection::call_reducer;
use crate::stdb::Coordinator;
use crate::world::{Actor, ChatOutcome, SpeechStore};

impl SpeechStore for Coordinator {
    /// Say, yell or `/e` on the speaker's Home Shard. The Module's language Gate answers a
    /// `chat:*` Refusal.
    fn send_chat(
        &self,
        actor: Actor,
        chat_type: u8,
        language: u8,
        message: String,
    ) -> Result<ChatOutcome> {
        chat_outcome(call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_send_chat",
            gw_send_chat_then(self.session_actor(actor), chat_type, language, message)
        ))
    }

    fn send_emote(
        &self,
        actor: Actor,
        text_emote: u32,
        emote_anim: u32,
        target_guid: u64,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
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
    /// the Home Shard through one Store operation. A Refusal carries the text the GM reads: the
    /// Module's reason, or the Gateway's own when the Account is unknown. Realm-core being
    /// unreachable is a Transport Loss.
    fn gm_command(&self, account_name: &str, actor: Actor, text: String) -> Result<()> {
        crate::realm_core::run_gm_command(self, account_name, actor.guid(), text)
    }
}
