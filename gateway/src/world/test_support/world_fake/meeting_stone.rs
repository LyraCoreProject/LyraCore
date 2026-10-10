use super::super::*;

/// The Meeting Stone family is tested against its own Fake in `handlers/meeting_stone.rs`. Here no
/// stone exists and nobody is queued.
impl MeetingStoneActionStore for WorldFake {
    fn admit_meeting_stone(&self, _actor_guid: u64, _go_guid: u64) -> Result<MeetingStoneOutcome> {
        Ok(MeetingStoneOutcome::Refused(
            lyracore_shared::meeting_stone::MeetingStoneRefusal::NotAMeetingStone,
        ))
    }

    fn meeting_stone_area(&self, _go_guid: u64) -> Result<Option<u32>> {
        Ok(None)
    }

    fn party_members(&self, _actor_guid: u64) -> Result<Option<Vec<u64>>> {
        Ok(None)
    }

    fn seeker_facts(&self, _character_guid: u64) -> Result<Option<crate::world::SeekerFacts>> {
        Ok(None)
    }

    fn meeting_stone_op(
        &self,
        _actor_guid: u64,
        _op: u8,
        _area_id: u32,
        _seekers: Vec<crate::world::SeekerFacts>,
    ) -> Result<MeetingStoneOutcome> {
        Ok(MeetingStoneOutcome::Ran)
    }

    fn queued_area(&self, _character_guid: u64) -> Result<Option<u32>> {
        Ok(None)
    }
}
