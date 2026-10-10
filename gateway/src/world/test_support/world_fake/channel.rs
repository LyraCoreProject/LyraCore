use super::super::*;

#[derive(Default)]
pub(crate) struct ChannelState {
    /// What `channel_op` answers. `None` succeeds.
    pub(crate) channel_outcome: Option<ChannelOutcome>,
    /// Recorded `channel_op` calls: `(actor_guid, op, request)`.
    pub(crate) channel_ops: std::sync::Mutex<Vec<(u64, u8, ChannelRequest)>>,
}

impl ChannelActionStore for WorldFake {
    fn channel_op(
        &self,
        actor_guid: u64,
        op: u8,
        request: ChannelRequest,
    ) -> Result<ChannelOutcome> {
        self.channel
            .channel_ops
            .lock()
            .unwrap()
            .push((actor_guid, op, request));
        Ok(self.channel.channel_outcome.unwrap_or(ChannelOutcome::Done))
    }

    fn channel_roster(&self, _team: u32, _channel_name: &str) -> Result<Option<ChannelRoster>> {
        Ok(None)
    }

    fn online_character_by_name(&self, name: &str) -> Result<Option<ResolvedTarget>> {
        resolve_online_character(self, name)
    }

    fn ignores(&self, owner_guid: u64, other_guid: u64) -> Result<bool> {
        whisper::ignored_anywhere(self, owner_guid, other_guid)
    }
}
