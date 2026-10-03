//! Meeting Stone Queue messages. `SMSG_MEETINGSTONE_COMPLETE` and `_IN_PROGRESS` are empty, so
//! they need no builder.

use wow_world_messages::vanilla::{
    Area, MSG_LOOKING_FOR_GROUP_Server, MeetingStoneFailure, MeetingStoneStatus,
    SMSG_MEETINGSTONE_JOINFAILED, SMSG_MEETINGSTONE_MEMBER_ADDED, SMSG_MEETINGSTONE_SETQUEUE,
};
use wow_world_messages::Guid;

/// `None` when `wow_world_messages` has no `Area` for the id or knows no such status.
pub fn build_meetingstone_setqueue(area_id: u32, status: u8) -> Option<SMSG_MEETINGSTONE_SETQUEUE> {
    Some(SMSG_MEETINGSTONE_SETQUEUE {
        area: Area::try_from(area_id).ok()?,
        status: MeetingStoneStatus::try_from(status).ok()?,
    })
}

/// `None` for a reason byte outside 1 to 3.
pub fn build_meetingstone_joinfailed(reason: u8) -> Option<SMSG_MEETINGSTONE_JOINFAILED> {
    Some(SMSG_MEETINGSTONE_JOINFAILED {
        reason: MeetingStoneFailure::try_from(reason).ok()?,
    })
}

/// A Stone Add, told to the members already in the Party.
pub fn build_meetingstone_member_added(guid: u64) -> SMSG_MEETINGSTONE_MEMBER_ADDED {
    SMSG_MEETINGSTONE_MEMBER_ADDED {
        guid: Guid::new(guid),
    }
}

/// The answer to `MSG_LOOKING_FOR_GROUP`. vmangos sends 0 (vm:Handlers/MiscHandler.cpp:277-282),
/// and the stock UI never reads it.
pub fn build_looking_for_group() -> MSG_LOOKING_FOR_GROUP_Server {
    MSG_LOOKING_FOR_GROUP_Server { unknown1: 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyracore_shared::meeting_stone::{join_failure, queue_status};
    use wow_world_messages::vanilla::ServerMessage;

    fn body(message: &impl ServerMessage) -> Vec<u8> {
        let mut framed = Vec::new();
        message.write_unencrypted_server(&mut framed).unwrap();
        framed.split_off(4)
    }

    /// cm:LFG/LFGHandler.cpp:146-152 writes the area as a u32 and the status as a u8.
    #[test]
    fn setqueue_is_the_area_then_the_status_byte() {
        let packet = build_meetingstone_setqueue(1581, queue_status::JOINED_QUEUE).unwrap();
        assert_eq!(body(&packet), [0x2D, 0x06, 0, 0, 1]);
        let none = build_meetingstone_setqueue(0, queue_status::NONE).unwrap();
        assert_eq!(body(&none), [0, 0, 0, 0, 5]);
        assert!(build_meetingstone_setqueue(1581, 6).is_none());
    }

    /// The shared status and failure bytes are the `wow_world_messages` values.
    #[test]
    fn the_shared_bytes_match_the_wire_enums() {
        use queue_status::*;
        for (byte, status) in [
            (LEAVE_QUEUE, MeetingStoneStatus::LeaveQueue),
            (JOINED_QUEUE, MeetingStoneStatus::JoinedQueue),
            (
                PARTY_MEMBER_LEFT_LFG,
                MeetingStoneStatus::PartyMemberLeftLfg,
            ),
            (
                PARTY_MEMBER_REMOVED_PARTY_REMOVED,
                MeetingStoneStatus::PartyMemberRemovedPartyRemoved,
            ),
            (
                LOOKING_FOR_NEW_PARTY_IN_QUEUE,
                MeetingStoneStatus::LookingForNewPartyInQueue,
            ),
            (NONE, MeetingStoneStatus::None),
        ] {
            assert_eq!(status.as_int(), byte);
        }
        for (byte, failure) in [
            (
                join_failure::PARTY_LEADER,
                MeetingStoneFailure::MeetingstoneFailPartyleader,
            ),
            (
                join_failure::FULL_GROUP,
                MeetingStoneFailure::MeetingstoneFailFullGroup,
            ),
            (
                join_failure::RAID_GROUP,
                MeetingStoneFailure::MeetingstoneFailRaidGroup,
            ),
        ] {
            assert_eq!(failure.as_int(), byte);
            assert_eq!(body(&build_meetingstone_joinfailed(byte).unwrap()), [byte]);
        }
        assert!(build_meetingstone_joinfailed(0).is_none());
    }

    #[test]
    fn member_added_is_the_guid_and_looking_for_group_is_zero() {
        assert_eq!(
            body(&build_meetingstone_member_added(0x0102)),
            0x0102u64.to_le_bytes()
        );
        assert_eq!(body(&build_looking_for_group()), [0, 0, 0, 0]);
    }
}
