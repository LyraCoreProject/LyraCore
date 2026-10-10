use super::super::*;

#[derive(Default)]
pub(crate) struct SpeechState {
    /// What `send_chat` answers; `None` delivers.
    pub(crate) send_chat_outcome: Option<ChatOutcome>,
    /// When set, `gm_command` returns this error — e.g. `"permission denied"` to
    /// drive the Say-handler's `Err` → self-only `SMSG_MESSAGECHAT` System relay.
    pub(crate) gm_command_error: Option<String>,
    /// Recorded `gm_command` dispatches — the dot-command divert test asserts the
    /// RIGHT raw text (still carrying its leading `.`) reached the reducer call, and that a NON-dot
    /// Say never reaches this vec at all.
    pub(crate) gm_commands: std::sync::Mutex<Vec<(String, String)>>,
    /// Current Realm-core Alpha Test Tools answer for the next command. `None` leaves the older
    /// fixed-outcome fixture in place; tests that set it model the production Store's fresh read.
    pub(crate) gm_alpha_test_tools: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// The authority conveyed with each command when `gm_alpha_test_tools` is in use.
    pub(crate) gm_authority_results: std::sync::Mutex<Vec<bool>>,
    /// Home Shard gameplay accepted by the focused Alpha Test Tools Fake. The command parser and
    /// effects belong to Module tests, so this only records the visible Store outcome.
    pub(crate) gm_gameplay_changes: std::sync::Mutex<Vec<String>>,
    /// Recorded `send_chat` lines: (chat_type, language, message).
    pub(crate) chats: std::sync::Mutex<Vec<(u8, u8, String)>>,
}

impl SpeechStore for WorldFake {
    fn send_chat(
        &self,
        _account_id: u64,
        _self_guid: u64,
        chat_type: u8,
        language: u8,
        message: String,
    ) -> Result<ChatOutcome> {
        // Recorded per SHARD like every other player-scoped call, so the partition rule (say/
        // yell stay shard-local and range-scoped) is assertable rather than merely stated.
        self.rec("send_chat");
        self.speech
            .chats
            .lock()
            .unwrap()
            .push((chat_type, language, message));
        Ok(self
            .speech
            .send_chat_outcome
            .unwrap_or(ChatOutcome::Delivered))
    }

    fn send_emote(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _text_emote: u32,
        _emote_anim: u32,
        _target_guid: u64,
    ) -> Result<()> {
        self.rec("send_emote");
        Ok(())
    }

    fn gm_command(&self, account_name: &str, _self_guid: u64, text: String) -> Result<()> {
        if let Some(alpha_test_tools) = &self.speech.gm_alpha_test_tools {
            let authorized = alpha_test_tools.load(std::sync::atomic::Ordering::SeqCst);
            self.speech
                .gm_commands
                .lock()
                .unwrap()
                .push((account_name.to_string(), text.clone()));
            self.speech
                .gm_authority_results
                .lock()
                .unwrap()
                .push(authorized);
            if authorized && (text.starts_with(".speed") || text.starts_with(".tele")) {
                self.speech.gm_gameplay_changes.lock().unwrap().push(text);
                return Ok(());
            }
            return Err(anyhow!("permission denied"));
        }
        match &self.speech.gm_command_error {
            Some(e) => Err(anyhow!("{e}")),
            None => {
                self.speech
                    .gm_commands
                    .lock()
                    .unwrap()
                    .push((account_name.to_string(), text));
                Ok(())
            }
        }
    }
}
