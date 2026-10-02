//! The Meeting Stone Queue wire contract: the `realm_meeting_stone_op` op bytes, the queue status
//! and join failure bytes the client renders, the typed [`MeetingStoneRefusal`], and the event kinds
//! and payload the Module writes to `game_group_event`. Both the Module and the Gateway import it.

/// The `op` byte of `realm_meeting_stone_op`. Its own byte space, separate from
/// [`crate::group::realm_op`], because the queue has its own reducer.
pub mod realm_op {
    /// `CMSG_MEETINGSTONE_JOIN`: queue the actor, or the Party it leads, for `area_id`.
    pub const JOIN: u8 = 0;
    /// `CMSG_MEETINGSTONE_LEAVE`: leave the queue.
    pub const LEAVE: u8 = 1;
}

/// `SMSG_MEETINGSTONE_SETQUEUE`'s status byte (cm:LFG/LFGDefines.h:50-58).
pub mod queue_status {
    pub const LEAVE_QUEUE: u8 = 0;
    pub const JOINED_QUEUE: u8 = 1;
    pub const PARTY_MEMBER_LEFT_LFG: u8 = 2;
    pub const PARTY_MEMBER_REMOVED_PARTY_REMOVED: u8 = 3;
    pub const LOOKING_FOR_NEW_PARTY_IN_QUEUE: u8 = 4;
    pub const NONE: u8 = 5;
}

/// `SMSG_MEETINGSTONE_JOINFAILED`'s reason byte (cm:LFG/LFGDefines.h:61-67).
pub mod join_failure {
    pub const PARTY_LEADER: u8 = 1;
    pub const FULL_GROUP: u8 = 2;
    pub const RAID_GROUP: u8 = 3;
}

/// The `game_group_event` kinds the Meeting Stone Queue writes. They share the byte space that
/// [`crate::group::event_kind`] documents.
pub mod event_kind {
    /// `SMSG_MEETINGSTONE_SETQUEUE`. Payload: [`super::encode_queue`].
    pub const QUEUE: u8 = 27;
    /// `SMSG_MEETINGSTONE_MEMBER_ADDED`. `other_guid` is the added Character.
    pub const MEMBER_ADDED: u8 = 28;
    /// `SMSG_MEETINGSTONE_IN_PROGRESS`. No payload.
    pub const IN_PROGRESS: u8 = 29;
    /// `SMSG_MEETINGSTONE_COMPLETE`. No payload.
    pub const COMPLETE: u8 = 30;
}

/// Why the Module refused a Meeting Stone Durable Request. The tag is the whole reducer error text.
/// Only the three party Refusals have a client answer ([`join_failure_for`]); both cores drop the
/// rest silently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeetingStoneRefusal {
    /// The actor is in a Party it does not lead.
    NotLeader,
    /// The actor leads a Raid.
    RaidGroup,
    /// The actor's Party already has five members.
    PartyFull,
    /// The actor has no World Session, no live entity, or is on a taxi flight.
    ActorUnavailable,
    /// The GameObject is gone, is not type 23, or has no imported stone row.
    NotAMeetingStone,
    /// The stone is out of reach.
    OutOfRange,
    /// The stone is on another map or in another instance.
    OtherPartition,
    /// The actor's level is outside the stone's range.
    LevelOutOfRange,
    /// The stone names no dungeon area.
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

/// The `SMSG_MEETINGSTONE_JOINFAILED` reason for a Refusal, or `None` when the client gets nothing
/// (cm:LFG/LFGHandler.cpp:58-77).
pub fn join_failure_for(refusal: MeetingStoneRefusal) -> Option<u8> {
    match refusal {
        MeetingStoneRefusal::NotLeader => Some(join_failure::PARTY_LEADER),
        MeetingStoneRefusal::PartyFull => Some(join_failure::FULL_GROUP),
        MeetingStoneRefusal::RaidGroup => Some(join_failure::RAID_GROUP),
        _ => None,
    }
}

/// `area,status` in decimal, the [`event_kind::QUEUE`] payload.
pub fn encode_queue(area_id: u32, status: u8) -> String {
    format!("{area_id},{status}")
}

/// `None` for a malformed payload or a status the client does not know.
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

    /// The three party Refusals answer with cmangos's failure byte; every other one is silent.
    #[test]
    fn only_the_party_refusals_answer_the_client() {
        use MeetingStoneRefusal::*;
        let answered: Vec<_> = MeetingStoneRefusal::ALL
            .into_iter()
            .filter_map(|refusal| join_failure_for(refusal).map(|byte| (refusal, byte)))
            .collect();
        assert_eq!(answered, [(NotLeader, 1), (RaidGroup, 3), (PartyFull, 2)]);
    }

    /// cm:LFG/LFGDefines.h:50-67.
    #[test]
    fn the_status_and_failure_bytes_are_the_vanilla_values() {
        use queue_status::*;
        assert_eq!(
            [
                LEAVE_QUEUE,
                JOINED_QUEUE,
                PARTY_MEMBER_LEFT_LFG,
                PARTY_MEMBER_REMOVED_PARTY_REMOVED,
                LOOKING_FOR_NEW_PARTY_IN_QUEUE,
                NONE
            ],
            [0, 1, 2, 3, 4, 5]
        );
        assert_eq!(
            [
                join_failure::PARTY_LEADER,
                join_failure::FULL_GROUP,
                join_failure::RAID_GROUP
            ],
            [1, 2, 3]
        );
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
