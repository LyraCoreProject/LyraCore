pub const GROUP_MAX_MEMBERS: usize = 5;

pub const RAID_MAX_MEMBERS: usize = 40;

pub const RAID_SUBGROUPS: u8 = 8;

pub const SUBGROUP_SIZE: usize = 5;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GroupKind {
    #[default]
    Party,
    Raid,
}

impl GroupKind {
    pub const fn wire(self) -> u8 {
        match self {
            Self::Party => 0,
            Self::Raid => 1,
        }
    }

    pub const fn from_wire(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Party),
            1 => Some(Self::Raid),
            _ => None,
        }
    }

    pub fn from_wire_or_default(byte: u8) -> Self {
        Self::from_wire(byte).unwrap_or_default()
    }

    pub const fn member_cap(self) -> usize {
        match self {
            Self::Party => GROUP_MAX_MEMBERS,
            Self::Raid => RAID_MAX_MEMBERS,
        }
    }
}

// SMSG_GROUP_LIST member flags encode subgroup | 0x80 for an Assistant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RaidSlot(u8);

impl RaidSlot {
    const ASSISTANT: u8 = 0x80;

    pub const fn new(subgroup: u8, assistant: bool) -> Option<Self> {
        if subgroup >= RAID_SUBGROUPS {
            return None;
        }
        Some(Self(if assistant {
            subgroup | Self::ASSISTANT
        } else {
            subgroup
        }))
    }

    pub fn for_raid_joiner(current: impl IntoIterator<Item = Self>) -> Option<Self> {
        let mut sizes = [0usize; RAID_SUBGROUPS as usize];
        for slot in current {
            sizes[usize::from(slot.subgroup())] += 1;
        }
        (0..RAID_SUBGROUPS)
            .find(|&subgroup| sizes[usize::from(subgroup)] < SUBGROUP_SIZE)
            .and_then(|subgroup| Self::new(subgroup, false))
    }

    pub const fn from_wire(byte: u8) -> Option<Self> {
        Self::new(byte & !Self::ASSISTANT, byte & Self::ASSISTANT != 0)
    }

    pub fn from_wire_or_default(byte: u8) -> Self {
        Self::from_wire(byte).unwrap_or_default()
    }

    pub const fn subgroup(self) -> u8 {
        self.0 & !Self::ASSISTANT
    }

    pub const fn is_assistant(self) -> bool {
        self.0 & Self::ASSISTANT != 0
    }

    pub const fn wire(self) -> u8 {
        self.0
    }

    pub const fn with_assistant(self, assistant: bool) -> Self {
        Self(if assistant {
            self.0 | Self::ASSISTANT
        } else {
            self.0 & !Self::ASSISTANT
        })
    }

    // destination_size excludes the mover, so its current Subgroup cannot refuse itself.
    pub fn moved_to_subgroup(
        self,
        destination: u8,
        destination_size: usize,
    ) -> Result<Option<Self>, GroupRefusal> {
        if destination >= RAID_SUBGROUPS {
            return Err(GroupRefusal::InvalidSubgroup);
        }
        if self.subgroup() == destination {
            return Ok(None);
        }
        if destination_size >= SUBGROUP_SIZE {
            return Err(GroupRefusal::SubgroupFull);
        }
        Ok(Some(Self::new(destination, self.is_assistant()).expect(
            "destination is below RAID_SUBGROUPS, checked above",
        )))
    }

    // An atomic swap cannot overfill either Subgroup.
    pub fn swapped_with(self, other: Self) -> Option<(Self, Self)> {
        if self.subgroup() == other.subgroup() {
            return None;
        }
        Some((
            Self::new(other.subgroup(), self.is_assistant())
                .expect("a stored Subgroup stays valid"),
            Self::new(self.subgroup(), other.is_assistant())
                .expect("a stored Subgroup stays valid"),
        ))
    }
}

// game_group_event kind 9 is retired and must not be reused.
pub mod event_kind {
    pub const INVITE: u8 = 0;
    // The roster payload and membership change commit in the same transaction.
    pub const LIST: u8 = 1;
    pub const DECLINE: u8 = 2;
    pub const DESTROYED: u8 = 3;
    pub const LOOT_ROLL_RESERVED_START: u8 = 4;
    // Send SMSG_GROUP_SET_LEADER before the LIST naming the new leader.
    pub const SET_LEADER: u8 = 20;
    pub const DEATH_DURABILITY: u8 = 13;
    pub const READY_CHECK: u8 = 21;
    pub const READY_CHECK_ANSWER: u8 = 22;
    pub const TARGET_ICON_UPDATE: u8 = 23;
    pub const TARGET_ICON_LIST: u8 = 24;
    pub const MINIMAP_PING: u8 = 25;
    pub const RANDOM_ROLL: u8 = 26;
    /// Empty `SMSG_GROUP_UNINVITE` to a member kicked from a surviving Group, before [`DESTROYED`].
    pub const UNINVITED: u8 = 31;
}

pub mod realm_op {
    pub const INVITE: u8 = 0;
    pub const ACCEPT: u8 = 1;
    pub const DECLINE: u8 = 2;
    pub const LEAVE: u8 = 3;
    pub const UNINVITE: u8 = 4;
    pub const LOOT_METHOD: u8 = 5;
    pub const RAID_CONVERT: u8 = 6;
    pub const SET_LEADER: u8 = 7;
    pub const SET_ASSISTANT: u8 = 8;
    pub const CHANGE_SUBGROUP: u8 = 9;
    pub const SWAP_SUBGROUP: u8 = 10;
    pub const READY_CHECK_START: u8 = 11;
    pub const READY_CHECK_ANSWER: u8 = 12;
    pub const TARGET_ICON: u8 = 13;
    pub const MINIMAP_PING: u8 = 14;
    pub const RANDOM_ROLL: u8 = 15;
}

pub mod leave_cause {
    pub const LEFT: u8 = 0;
    pub const CHARACTER_DELETED: u8 = 1;
}

// Older game_bot_invite_intent rows default to INVITE.
pub mod bot_op {
    pub const INVITE: u8 = 0;
    pub const LEAVE: u8 = 1;
}

pub const COMMAND_DISPATCH_LANES: u8 = 8;

pub const COMMAND_RESULT_WINDOW_MICROS: i64 = 30_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupRefusal {
    ActorUnavailable,
    InviteSelf,
    NoSuchPlayer,
    TargetOffline,
    AlreadyInGroup,
    GroupFull,
    NotLeader,
    NotInGroup,
    TargetNotInGroup,
    NoPendingInvite,
    InviterUnavailable,
    KickSelf,
    InvalidLootRules,
    IntentAlreadyClaimed,
    ActionSuppressed,
    WrongFaction,
    NotRaid,
    TargetIsSelf,
    InvalidSubgroup,
    SubgroupFull,
    InvalidTargetIcon,
}

impl GroupRefusal {
    pub const ALL: [Self; 21] = [
        Self::ActorUnavailable,
        Self::InviteSelf,
        Self::NoSuchPlayer,
        Self::TargetOffline,
        Self::AlreadyInGroup,
        Self::GroupFull,
        Self::NotLeader,
        Self::NotInGroup,
        Self::TargetNotInGroup,
        Self::NoPendingInvite,
        Self::InviterUnavailable,
        Self::KickSelf,
        Self::InvalidLootRules,
        Self::IntentAlreadyClaimed,
        Self::ActionSuppressed,
        Self::WrongFaction,
        Self::NotRaid,
        Self::TargetIsSelf,
        Self::InvalidSubgroup,
        Self::SubgroupFull,
        Self::InvalidTargetIcon,
    ];

    pub fn as_tag(self) -> &'static str {
        match self {
            Self::ActorUnavailable => "group:actor_unavailable",
            Self::InviteSelf => "group:invite_self",
            Self::NoSuchPlayer => "group:no_such_player",
            Self::TargetOffline => "group:target_offline",
            Self::AlreadyInGroup => "group:already_in_group",
            Self::GroupFull => "group:group_full",
            Self::NotLeader => "group:not_leader",
            Self::NotInGroup => "group:not_in_group",
            Self::TargetNotInGroup => "group:target_not_in_group",
            Self::NoPendingInvite => "group:no_pending_invite",
            Self::InviterUnavailable => "group:inviter_unavailable",
            Self::KickSelf => "group:kick_self",
            Self::InvalidLootRules => "group:invalid_loot_rules",
            Self::IntentAlreadyClaimed => "group:intent_already_claimed",
            Self::ActionSuppressed => "group:action_suppressed",
            Self::WrongFaction => "group:wrong_faction",
            Self::NotRaid => "group:not_raid",
            Self::TargetIsSelf => "group:target_is_self",
            Self::InvalidSubgroup => "group:invalid_subgroup",
            Self::SubgroupFull => "group:subgroup_full",
            Self::InvalidTargetIcon => "group:invalid_target_icon",
        }
    }

    pub fn parse_tag(tag: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|refusal| refusal.as_tag() == tag)
    }
}

pub const TARGET_ICON_COUNT: u8 = 8;

// MSG_RAID_TARGET_UPDATE index 0xFF requests the full list.
pub const TARGET_ICON_LIST_REQUEST: u8 = 0xFF;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetIcon {
    pub icon: u8,
    pub target_guid: u64,
}

impl TargetIcon {
    pub fn encode(self) -> String {
        format!("{},{}", self.icon, self.target_guid)
    }

    pub fn decode(payload: &str) -> Option<Self> {
        let (icon, target_guid) = payload.split_once(',')?;
        let icon = icon.parse().ok().filter(|&icon| icon < TARGET_ICON_COUNT)?;
        Some(Self {
            icon,
            target_guid: target_guid.parse().ok()?,
        })
    }
}

pub fn encode_target_icons(icons: &[TargetIcon]) -> String {
    icons
        .iter()
        .map(|icon| icon.encode())
        .collect::<Vec<_>>()
        .join(";")
}

pub fn decode_target_icons(payload: &str) -> Option<Vec<TargetIcon>> {
    let mut icons: Vec<TargetIcon> = Vec::new();
    for entry in payload.split(';').filter(|entry| !entry.is_empty()) {
        let icon = TargetIcon::decode(entry).filter(|icon| icon.target_guid != 0)?;
        if icons
            .iter()
            .any(|held| held.icon == icon.icon || held.target_guid == icon.target_guid)
        {
            return None;
        }
        icons.push(icon);
    }
    Some(icons)
}

pub fn encode_ready_check_answer(state: u8) -> String {
    state.to_string()
}

pub fn decode_ready_check_answer(payload: &str) -> Option<u8> {
    payload.parse().ok()
}

// Float bit patterns preserve the exact MINIMAP_PING coordinates.
pub fn encode_minimap_ping(x: f32, y: f32) -> String {
    format!("{},{}", x.to_bits(), y.to_bits())
}

pub fn decode_minimap_ping(payload: &str) -> Option<(f32, f32)> {
    let (x, y) = payload.split_once(',')?;
    Some((
        f32::from_bits(x.parse().ok()?),
        f32::from_bits(y.parse().ok()?),
    ))
}

pub fn encode_random_roll(min: u32, max: u32, result: u32) -> String {
    format!("{min},{max},{result}")
}

pub fn decode_random_roll(payload: &str) -> Option<(u32, u32, u32)> {
    let mut parts = payload.split(',');
    let min = parts.next()?.parse().ok()?;
    let max = parts.next()?.parse().ok()?;
    let result = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(min..=max).contains(&result) {
        return None;
    }
    Some((min, max, result))
}

// The roster commits with its change; the packet builder adds live presence and missing names.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RosterPayload {
    pub leader: u64,
    pub loot_method: u8,
    pub loot_threshold: u8,
    pub master_looter_guid: u64,
    pub kind: GroupKind,
    pub members: Vec<RosterMember>,
    // A Party clears Target Icons on SMSG_GROUP_LIST, so resend them after the list.
    pub target_icons: Vec<TargetIcon>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RosterMember {
    pub guid: u64,
    pub name: String,
    pub slot: RaidSlot,
}

impl RosterPayload {
    pub fn encode(&self) -> String {
        let members: Vec<String> = self
            .members
            .iter()
            .map(|member| {
                let name = member.name.replace(['|', ';', ','], "_");
                format!("{},{name},{}", member.guid, member.slot.wire())
            })
            .collect();
        let mut payload = format!(
            "{},{},{},{},{}|{}",
            self.leader,
            self.loot_method,
            self.loot_threshold,
            self.master_looter_guid,
            self.kind.wire(),
            members.join(";")
        );
        if !self.target_icons.is_empty() {
            payload.push('|');
            payload.push_str(&encode_target_icons(&self.target_icons));
        }
        payload
    }

    pub fn decode(payload: &str) -> Option<Self> {
        let mut segments = payload.split('|');
        let head = segments.next()?;
        let rest = segments.next()?;
        let target_icons = match segments.next() {
            Some(icons) => decode_target_icons(icons)?,
            None => Vec::new(),
        };
        if segments.next().is_some() {
            return None;
        }
        let mut head_parts = head.split(',');
        let leader = head_parts.next()?.parse().ok()?;
        let loot_method = head_parts.next()?.parse().ok()?;
        let loot_threshold = head_parts.next()?.parse().ok()?;
        let master_looter_guid = head_parts.next()?.parse().ok()?;
        let kind = GroupKind::from_wire(head_parts.next()?.parse().ok()?)?;
        if head_parts.next().is_some() {
            return None;
        }
        let mut members = Vec::new();
        for entry in rest.split(';').filter(|entry| !entry.is_empty()) {
            let mut parts = entry.split(',');
            let guid = parts.next()?.parse().ok()?;
            let name = parts.next()?.to_string();
            let slot = RaidSlot::from_wire(parts.next()?.parse().ok()?)?;
            if parts.next().is_some() {
                return None;
            }
            members.push(RosterMember { guid, name, slot });
        }
        if members.is_empty() || members.len() > RAID_MAX_MEMBERS {
            return None;
        }
        Some(Self {
            leader,
            loot_method,
            loot_threshold,
            master_looter_guid,
            kind,
            members,
            target_icons,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_group_event_kind_is_distinct_and_none_reuses_kind_9() {
        use crate::loot_roll::event_kind as roll;
        use crate::meeting_stone::event_kind as stone;
        use crate::quest::share_event_kind as share;
        let kinds = [
            event_kind::INVITE,
            event_kind::LIST,
            event_kind::DECLINE,
            event_kind::DESTROYED,
            roll::ROLL_START,
            roll::ROLL_VOTE,
            roll::ROLL_WON,
            roll::MASTER_LIST,
            roll::MONEY_SHARE,
            roll::ALL_PASSED,
            event_kind::DEATH_DURABILITY,
            share::QUEST_SHARE,
            share::QUEST_PUSH_RESULT,
            event_kind::SET_LEADER,
            event_kind::READY_CHECK,
            event_kind::READY_CHECK_ANSWER,
            event_kind::TARGET_ICON_UPDATE,
            event_kind::TARGET_ICON_LIST,
            event_kind::MINIMAP_PING,
            event_kind::RANDOM_ROLL,
            stone::QUEUE,
            stone::MEMBER_ADDED,
            stone::IN_PROGRESS,
            stone::COMPLETE,
        ];
        let mut sorted = kinds;
        sorted.sort_unstable();
        assert_eq!(
            sorted,
            [
                0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 13, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29,
                30
            ]
        );
        assert_eq!(event_kind::LOOT_ROLL_RESERVED_START, roll::ROLL_START);
    }

    #[test]
    fn every_group_refusal_tag_round_trips() {
        for refusal in GroupRefusal::ALL {
            assert_eq!(GroupRefusal::parse_tag(refusal.as_tag()), Some(refusal));
        }
        assert_eq!(GroupRefusal::parse_tag("group:"), None);
        assert_eq!(
            GroupRefusal::parse_tag("gw_group_invite reducer timed out after 10s"),
            None
        );
    }

    fn member(guid: u64, name: &str, subgroup: u8, assistant: bool) -> RosterMember {
        RosterMember {
            guid,
            name: name.to_string(),
            slot: RaidSlot::new(subgroup, assistant).unwrap(),
        }
    }

    #[test]
    fn a_raid_is_eight_subgroups_of_five_and_a_party_caps_at_five() {
        assert_eq!(GROUP_MAX_MEMBERS, 5);
        assert_eq!(RAID_MAX_MEMBERS, 40);
        assert_eq!(RAID_SUBGROUPS, 8);
        assert_eq!(SUBGROUP_SIZE, 5);
        assert_eq!(GroupKind::Party.member_cap(), 5);
        assert_eq!(GroupKind::Raid.member_cap(), 40);
    }

    #[test]
    fn the_group_kind_byte_is_the_vanilla_group_type() {
        assert_eq!(GroupKind::default(), GroupKind::Party);
        assert_eq!(GroupKind::Party.wire(), 0);
        assert_eq!(GroupKind::Raid.wire(), 1);
        assert_eq!(GroupKind::from_wire(0), Some(GroupKind::Party));
        assert_eq!(GroupKind::from_wire(1), Some(GroupKind::Raid));
        assert_eq!(GroupKind::from_wire(2), None);
    }

    #[test]
    fn a_raid_slot_is_the_vanilla_member_flags_byte() {
        assert_eq!(RaidSlot::default().wire(), 0);
        assert_eq!(RaidSlot::new(0, false).unwrap().wire(), 0x00);
        assert_eq!(RaidSlot::new(1, false).unwrap().wire(), 0x01);
        assert_eq!(RaidSlot::new(1, true).unwrap().wire(), 0x81);
        assert_eq!(RaidSlot::new(7, true).unwrap().wire(), 0x87);
        let slot = RaidSlot::from_wire(0x83).unwrap();
        assert_eq!(slot.subgroup(), 3);
        assert!(slot.is_assistant());
        assert!(!RaidSlot::from_wire(0x03).unwrap().is_assistant());
    }

    fn raid_of(sizes: [u8; 8]) -> Vec<RaidSlot> {
        (0u8..8)
            .flat_map(|subgroup| {
                (0..sizes[usize::from(subgroup)])
                    .map(move |index| RaidSlot::new(subgroup, index == 0).unwrap())
            })
            .collect()
    }

    #[test]
    fn a_raid_joiner_takes_the_first_subgroup_with_room() {
        let joiner = |sizes| RaidSlot::for_raid_joiner(raid_of(sizes));
        let slot = |subgroup| RaidSlot::new(subgroup, false);
        assert_eq!(joiner([5, 5, 3, 0, 0, 0, 0, 0]), slot(2));
        assert_eq!(joiner([0; 8]), slot(0));
        assert_eq!(joiner([5, 4, 0, 0, 0, 0, 0, 0]), slot(1));
        assert_eq!(joiner([5, 5, 5, 5, 5, 5, 5, 4]), slot(7));
        assert_eq!(
            joiner([5, 0, 5, 0, 0, 0, 0, 0]),
            slot(1),
            "a gap is filled first"
        );
        assert_eq!(joiner([5; 8]), None);
    }

    #[test]
    fn a_raid_slot_refuses_a_ninth_subgroup_and_unknown_bits() {
        assert_eq!(RaidSlot::new(8, false), None);
        assert_eq!(RaidSlot::new(255, true), None);
        for byte in [0x08, 0x10, 0x40, 0x88, 0xFF] {
            assert_eq!(RaidSlot::from_wire(byte), None, "{byte:#04x}");
        }
    }

    #[test]
    fn a_subgroup_move_refuses_a_full_or_out_of_range_destination() {
        let mover = RaidSlot::new(0, true).unwrap();
        assert_eq!(
            mover.moved_to_subgroup(2, 4),
            Ok(Some(RaidSlot::new(2, true).unwrap())),
            "room in the destination, the Assistant bit rides along"
        );
        assert_eq!(
            mover.moved_to_subgroup(2, 5),
            Err(GroupRefusal::SubgroupFull),
            "AC 3: five members already fill the destination"
        );
        assert_eq!(
            mover.moved_to_subgroup(8, 0),
            Err(GroupRefusal::InvalidSubgroup),
            "AC 4: a raid has only 8 Subgroups, 0 to 7"
        );
    }

    #[test]
    fn a_subgroup_move_to_the_same_subgroup_is_a_no_op() {
        let mover = RaidSlot::new(3, false).unwrap();
        assert_eq!(mover.moved_to_subgroup(3, 5), Ok(None));
    }

    #[test]
    fn a_subgroup_swap_needs_no_capacity_gate() {
        let first = RaidSlot::new(0, true).unwrap();
        let second = RaidSlot::new(1, false).unwrap();
        assert_eq!(
            first.swapped_with(second),
            Some((
                RaidSlot::new(1, true).unwrap(),
                RaidSlot::new(0, false).unwrap()
            )),
            "each Subgroup exchanges, each Assistant bit stays with its member"
        );
    }

    #[test]
    fn a_subgroup_swap_within_one_subgroup_is_a_no_op() {
        let first = RaidSlot::new(4, true).unwrap();
        let second = RaidSlot::new(4, false).unwrap();
        assert_eq!(first.swapped_with(second), None);
    }

    #[test]
    fn a_raid_roster_encodes_its_kind_and_slots_and_strips_hostile_names() {
        let roster = RosterPayload {
            leader: 2,
            loot_method: 3,
            loot_threshold: 2,
            master_looter_guid: 0,
            kind: GroupKind::Raid,
            members: vec![
                member(2, "Ginger", 0, false),
                member(3, "df|s;d,fsd", 1, true),
            ],
            target_icons: Vec::new(),
        };
        let wire = roster.encode();
        assert_eq!(wire, "2,3,2,0,1|2,Ginger,0;3,df_s_d_fsd,129");
        let decoded = RosterPayload::decode(&wire).unwrap();
        assert_eq!(decoded.kind, GroupKind::Raid);
        assert_eq!(decoded.members[1], member(3, "df_s_d_fsd", 1, true));
        assert_eq!(
            RosterPayload {
                members: vec![
                    member(2, "Ginger", 0, false),
                    member(3, "df_s_d_fsd", 1, true)
                ],
                ..roster
            },
            decoded
        );
    }

    #[test]
    fn a_full_raid_round_trips_and_a_forty_first_member_fails_closed() {
        let mut roster = RosterPayload {
            leader: 1,
            loot_method: 2,
            loot_threshold: 4,
            master_looter_guid: 7,
            kind: GroupKind::Raid,
            members: (0..RAID_MAX_MEMBERS as u64)
                .map(|index| member(index + 1, &format!("M{index}"), (index / 5) as u8, false))
                .collect(),
            target_icons: Vec::new(),
        };
        assert_eq!(
            RosterPayload::decode(&roster.encode()),
            Some(roster.clone())
        );
        roster.members.push(member(41, "Extra", 7, false));
        assert_eq!(RosterPayload::decode(&roster.encode()), None);
    }

    #[test]
    fn roster_decode_fails_closed_on_garbage() {
        for payload in [
            "",
            "x,3,2,0,0|1,A,0",
            "2,3,2,0,0|",
            "2|1,A,0",
            "2,3,2,0|1,A,1",
            "2,3,2,0,2|1,A,0",
            "2,3,2,0,0,9|1,A,0",
            "2,3,2,0,0|x,Bob,0",
            "2,3,2,0,0|5,Bob",
            "2,3,2,0,0|5",
            "2,3,2,0,0|5,Bob,8",
            "2,3,2,0,0|5,Bob,maybe",
            "2,3,2,0,0|5,Bob,0,1",
            "2,3,2,0,0|5,Bob,0|7",
            "2,3,2,0,0|5,Bob,0|8,99",
            "2,3,2,0,0|5,Bob,0|7,99;7,98",
            "2,3,2,0,0|5,Bob,0|7,99;6,99",
            "2,3,2,0,0|5,Bob,0|7,0",
            "2,3,2,0,0|5,Bob,0|7,99|6,98",
        ] {
            assert_eq!(RosterPayload::decode(payload), None, "{payload:?}");
        }
    }

    #[test]
    fn bot_group_intent_op_codes_are_stable_and_distinct() {
        assert_eq!(bot_op::INVITE, 0, "the END-appended column defaults to 0");
        assert_eq!(bot_op::LEAVE, 1);
        assert_ne!(bot_op::INVITE, bot_op::LEAVE);
    }

    #[test]
    fn realm_group_op_codes_are_stable_and_distinct() {
        assert_eq!(realm_op::INVITE, 0);
        assert_eq!(realm_op::ACCEPT, 1);
        assert_eq!(realm_op::DECLINE, 2);
        assert_eq!(realm_op::LEAVE, 3);
        assert_eq!(realm_op::UNINVITE, 4);
        assert_eq!(realm_op::LOOT_METHOD, 5);
        assert_eq!(realm_op::RAID_CONVERT, 6);
        assert_eq!(realm_op::SET_LEADER, 7);
        assert_eq!(realm_op::SET_ASSISTANT, 8);
        assert_eq!(realm_op::CHANGE_SUBGROUP, 9);
        assert_eq!(realm_op::SWAP_SUBGROUP, 10);
        assert_eq!(realm_op::READY_CHECK_START, 11);
        assert_eq!(realm_op::READY_CHECK_ANSWER, 12);
        assert_eq!(realm_op::TARGET_ICON, 13);
        assert_eq!(realm_op::MINIMAP_PING, 14);
        assert_eq!(realm_op::RANDOM_ROLL, 15);
        let all = [
            realm_op::INVITE,
            realm_op::ACCEPT,
            realm_op::DECLINE,
            realm_op::LEAVE,
            realm_op::UNINVITE,
            realm_op::LOOT_METHOD,
            realm_op::RAID_CONVERT,
            realm_op::SET_LEADER,
            realm_op::SET_ASSISTANT,
            realm_op::CHANGE_SUBGROUP,
            realm_op::SWAP_SUBGROUP,
            realm_op::READY_CHECK_START,
            realm_op::READY_CHECK_ANSWER,
            realm_op::TARGET_ICON,
            realm_op::MINIMAP_PING,
            realm_op::RANDOM_ROLL,
        ];
        let mut sorted = all.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            all.len(),
            "two realm group ops share an op byte"
        );
    }

    #[test]
    fn promoting_or_demoting_keeps_the_subgroup() {
        let plain = RaidSlot::new(3, false).unwrap();
        assert_eq!(plain.with_assistant(true).wire(), 0x83);
        assert_eq!(plain.with_assistant(true).with_assistant(false), plain);
        assert_eq!(plain.with_assistant(false), plain);
    }

    #[test]
    fn the_leader_announcement_kind_is_20_and_distinct() {
        use crate::loot_roll::event_kind as roll;
        use crate::quest::share_event_kind as share;
        assert_eq!(event_kind::SET_LEADER, 20);
        let mut kinds = vec![
            event_kind::INVITE,
            event_kind::LIST,
            event_kind::DECLINE,
            event_kind::DESTROYED,
            roll::ROLL_START,
            roll::ROLL_VOTE,
            roll::ROLL_WON,
            roll::MASTER_LIST,
            roll::MONEY_SHARE,
            share::QUEST_SHARE,
            share::QUEST_PUSH_RESULT,
            event_kind::SET_LEADER,
        ];
        kinds.sort_unstable();
        kinds.dedup();
        assert_eq!(kinds, [0u8, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 20]);
    }

    #[test]
    fn the_target_is_self_refusal_has_its_own_tag() {
        assert_eq!(GroupRefusal::TargetIsSelf.as_tag(), "group:target_is_self");
    }

    #[test]
    fn group_broadcast_kinds_are_pinned_and_distinct_from_every_other_kind() {
        use crate::loot_roll::event_kind as roll;
        use crate::quest::share_event_kind as share;
        assert_eq!(event_kind::READY_CHECK, 21);
        assert_eq!(event_kind::READY_CHECK_ANSWER, 22);
        assert_eq!(event_kind::TARGET_ICON_UPDATE, 23);
        assert_eq!(event_kind::TARGET_ICON_LIST, 24);
        assert_eq!(event_kind::MINIMAP_PING, 25);
        assert_eq!(event_kind::RANDOM_ROLL, 26);
        let all = [
            event_kind::INVITE,
            event_kind::LIST,
            event_kind::DECLINE,
            event_kind::DESTROYED,
            roll::ROLL_START,
            roll::ROLL_VOTE,
            roll::ROLL_WON,
            roll::MASTER_LIST,
            roll::MONEY_SHARE,
            share::QUEST_SHARE,
            share::QUEST_PUSH_RESULT,
            event_kind::SET_LEADER,
            event_kind::READY_CHECK,
            event_kind::READY_CHECK_ANSWER,
            event_kind::TARGET_ICON_UPDATE,
            event_kind::TARGET_ICON_LIST,
            event_kind::MINIMAP_PING,
            event_kind::RANDOM_ROLL,
        ];
        let mut sorted = all.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            all.len(),
            "two group event kinds share a byte"
        );
        assert!(
            !all.contains(&9),
            "kind 9 was party chat and is never reused"
        );
    }

    #[test]
    fn a_plain_leave_is_cause_zero_and_a_deleted_character_is_one() {
        assert_eq!(leave_cause::LEFT, 0);
        assert_eq!(leave_cause::CHARACTER_DELETED, 1);
    }

    #[test]
    fn the_invalid_target_icon_refusal_has_its_own_tag() {
        assert_eq!(
            GroupRefusal::InvalidTargetIcon.as_tag(),
            "group:invalid_target_icon"
        );
    }

    fn icon(icon: u8, target_guid: u64) -> TargetIcon {
        TargetIcon { icon, target_guid }
    }

    #[test]
    fn a_group_holds_eight_target_icons_and_0xff_asks_for_the_list() {
        assert_eq!(TARGET_ICON_COUNT, 8);
        assert_eq!(TARGET_ICON_LIST_REQUEST, 0xFF);
    }

    #[test]
    fn a_party_roster_carries_its_target_icons_after_the_members() {
        let mut roster = RosterPayload {
            leader: 2,
            loot_method: 3,
            loot_threshold: 2,
            master_looter_guid: 0,
            kind: GroupKind::Party,
            members: vec![member(2, "Ginger", 0, false), member(3, "Vim", 0, false)],
            target_icons: vec![icon(7, 900), icon(0, 901)],
        };
        let wire = roster.encode();
        assert_eq!(wire, "2,3,2,0,0|2,Ginger,0;3,Vim,0|7,900;0,901");
        assert_eq!(RosterPayload::decode(&wire), Some(roster.clone()));
        roster.target_icons.clear();
        assert_eq!(roster.encode(), "2,3,2,0,0|2,Ginger,0;3,Vim,0");
    }

    #[test]
    fn a_target_icon_update_round_trips_a_set_and_a_clear() {
        assert_eq!(
            icon(7, 0xF130_0000_0000_0042).encode(),
            "7,17379390962022744130"
        );
        for update in [icon(7, 0xF130_0000_0000_0042), icon(0, 0)] {
            assert_eq!(TargetIcon::decode(&update.encode()), Some(update));
        }
        for payload in ["", "7", "8,1", "255,1", "x,1", "7,x", "7,-1", "7,1,2"] {
            assert_eq!(TargetIcon::decode(payload), None, "{payload:?}");
        }
    }

    #[test]
    fn a_target_icon_list_round_trips_and_an_empty_list_is_empty() {
        let icons = vec![icon(0, 11), icon(7, 12)];
        assert_eq!(encode_target_icons(&icons), "0,11;7,12");
        assert_eq!(decode_target_icons("0,11;7,12"), Some(icons));
        assert_eq!(encode_target_icons(&[]), "");
        assert_eq!(decode_target_icons(""), Some(Vec::new()));
        for payload in ["0,11;0,12", "0,11;1,11", "0,0", "8,1", "0,11;x"] {
            assert_eq!(decode_target_icons(payload), None, "{payload:?}");
        }
    }

    #[test]
    fn a_ready_check_answer_carries_the_state_byte() {
        for state in [0u8, 1, 255] {
            assert_eq!(
                decode_ready_check_answer(&encode_ready_check_answer(state)),
                Some(state)
            );
        }
        assert_eq!(encode_ready_check_answer(1), "1");
        for payload in ["", "256", "-1", "yes"] {
            assert_eq!(decode_ready_check_answer(payload), None, "{payload:?}");
        }
    }

    #[test]
    fn a_minimap_ping_carries_each_float_bit_for_bit() {
        assert_eq!(encode_minimap_ping(0.5, -0.25), "1056964608,3196059648");
        for (x, y) in [
            (0.5f32, -0.25f32),
            (-1234.567, 0.1),
            (f32::MIN_POSITIVE, -0.0),
            (f32::MAX, f32::MIN),
        ] {
            let (dx, dy) = decode_minimap_ping(&encode_minimap_ping(x, y)).unwrap();
            assert_eq!((dx.to_bits(), dy.to_bits()), (x.to_bits(), y.to_bits()));
        }
        for payload in ["", "1", "1,x", "1,2,3", "4294967296,0"] {
            assert_eq!(decode_minimap_ping(payload), None, "{payload:?}");
        }
    }

    #[test]
    fn a_random_roll_carries_its_range_and_result() {
        assert_eq!(encode_random_roll(1, 100, 42), "1,100,42");
        assert_eq!(decode_random_roll("1,100,42"), Some((1, 100, 42)));
        assert_eq!(decode_random_roll("5,5,5"), Some((5, 5, 5)));
        for payload in ["", "1,100", "1,100,101", "5,10,4", "1,100,42,0", "1,x,2"] {
            assert_eq!(decode_random_roll(payload), None, "{payload:?}");
        }
    }
}
