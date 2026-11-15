pub mod realm_op {
    pub const JOIN: u8 = 0;
    pub const LEAVE: u8 = 1;
}

// SMSG_MEETINGSTONE_SETQUEUE status byte, cm:LFG/LFGDefines.h:50-58.
pub mod queue_status {
    pub const LEAVE_QUEUE: u8 = 0;
    pub const JOINED_QUEUE: u8 = 1;
    pub const PARTY_MEMBER_LEFT_LFG: u8 = 2;
    pub const PARTY_MEMBER_REMOVED_PARTY_REMOVED: u8 = 3;
    pub const LOOKING_FOR_NEW_PARTY_IN_QUEUE: u8 = 4;
    pub const NONE: u8 = 5;
}

// SMSG_MEETINGSTONE_JOINFAILED reason byte, cm:LFG/LFGDefines.h:61-67.
pub mod join_failure {
    pub const PARTY_LEADER: u8 = 1;
    pub const FULL_GROUP: u8 = 2;
    pub const RAID_GROUP: u8 = 3;
}

pub mod event_kind {
    pub const QUEUE: u8 = 27;
    pub const MEMBER_ADDED: u8 = 28;
    pub const IN_PROGRESS: u8 = 29;
    pub const COMPLETE: u8 = 30;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeetingStoneRefusal {
    NotLeader,
    RaidGroup,
    PartyFull,
    ActorUnavailable,
    NotAMeetingStone,
    OutOfRange,
    OtherPartition,
    LevelOutOfRange,
    UnknownArea,
}

impl MeetingStoneRefusal {
    pub const ALL: [Self; 9] = [
        Self::NotLeader,
        Self::RaidGroup,
        Self::PartyFull,
        Self::ActorUnavailable,
        Self::NotAMeetingStone,
        Self::OutOfRange,
        Self::OtherPartition,
        Self::LevelOutOfRange,
        Self::UnknownArea,
    ];

    pub fn as_tag(self) -> &'static str {
        match self {
            Self::NotLeader => "meeting_stone:not_leader",
            Self::RaidGroup => "meeting_stone:raid_group",
            Self::PartyFull => "meeting_stone:party_full",
            Self::ActorUnavailable => "meeting_stone:actor_unavailable",
            Self::NotAMeetingStone => "meeting_stone:not_a_meeting_stone",
            Self::OutOfRange => "meeting_stone:out_of_range",
            Self::OtherPartition => "meeting_stone:other_partition",
            Self::LevelOutOfRange => "meeting_stone:level_out_of_range",
            Self::UnknownArea => "meeting_stone:unknown_area",
        }
    }

    pub fn parse_tag(tag: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|refusal| refusal.as_tag() == tag)
    }
}

pub fn join_failure_for(refusal: MeetingStoneRefusal) -> Option<u8> {
    match refusal {
        MeetingStoneRefusal::NotLeader => Some(join_failure::PARTY_LEADER),
        MeetingStoneRefusal::PartyFull => Some(join_failure::FULL_GROUP),
        MeetingStoneRefusal::RaidGroup => Some(join_failure::RAID_GROUP),
        _ => None,
    }
}

pub fn encode_queue(area_id: u32, status: u8) -> String {
    format!("{area_id},{status}")
}

pub fn decode_queue(payload: &str) -> Option<(u32, u8)> {
    let (area, status) = payload.split_once(',')?;
    let status: u8 = status.parse().ok()?;
    if status > queue_status::NONE {
        return None;
    }
    Some((area.parse().ok()?, status))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_meeting_stone_refusal_tag_round_trips() {
        for refusal in MeetingStoneRefusal::ALL {
            assert!(refusal.as_tag().starts_with("meeting_stone:"));
            assert_eq!(
                MeetingStoneRefusal::parse_tag(refusal.as_tag()),
                Some(refusal)
            );
        }
        assert_eq!(MeetingStoneRefusal::parse_tag("meeting_stone:"), None);
        assert_eq!(MeetingStoneRefusal::parse_tag("group:not_leader"), None);
    }

    #[test]
    fn only_the_party_refusals_answer_the_client() {
        use MeetingStoneRefusal::*;
        let answered: Vec<_> = MeetingStoneRefusal::ALL
            .into_iter()
            .filter_map(|refusal| join_failure_for(refusal).map(|byte| (refusal, byte)))
            .collect();
        assert_eq!(answered, [(NotLeader, 1), (RaidGroup, 3), (PartyFull, 2)]);
    }

    #[test]
    fn a_queue_payload_round_trips_and_fails_closed() {
        assert_eq!(encode_queue(1581, queue_status::JOINED_QUEUE), "1581,1");
        assert_eq!(decode_queue("1581,1"), Some((1581, 1)));
        assert_eq!(decode_queue("0,5"), Some((0, 5)));
        for bad in ["1581,6", "1581", "1581,", ",1", "x,1", "1581,1,2", ""] {
            assert_eq!(decode_queue(bad), None, "{bad:?}");
        }
    }
}
