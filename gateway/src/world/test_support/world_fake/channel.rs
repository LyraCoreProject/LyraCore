use super::super::*;

impl ChannelActionStore for WorldFake {
    fn channel_op(
        &self,
        _actor: Actor,
        _op: u8,
        _request: ChannelRequest,
    ) -> Result<ChannelOutcome> {
        Ok(ChannelOutcome::Done)
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
