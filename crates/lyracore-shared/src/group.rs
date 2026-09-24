//! The module↔gateway GROUP wire contract — event-kind codes, the roster-payload grammar, and the
//! typed [`GroupRefusal`] the gateway turns into `PARTY_COMMAND_RESULT`. Cross-boundary constants
//! live HERE, both crates import: a module-side renumber, delimiter change, or new refusal becomes
//! a compile error on the gateway side instead of a runtime mismatch. Same precedent as
//! `type_mask`/`npc_flags`.

/// Vanilla party size (cm:Group.h:40). Realm authority and Gateway projections share this bound.
pub const GROUP_MAX_MEMBERS: usize = 5;

/// Vanilla raid size (cm:Group.h:41).
pub const RAID_MAX_MEMBERS: usize = 40;

/// A Raid has 8 Subgroups, numbered 0 to 7 (cm:Group.h:42).
pub const RAID_SUBGROUPS: u8 = 8;

/// Members in one Subgroup (cm:Group.h:42).
pub const SUBGROUP_SIZE: usize = 5;

/// A Group is a Party until its leader converts it to a Raid. It never converts back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GroupKind {
    #[default]
    Party,
    Raid,
}

impl GroupKind {
    /// The `SMSG_GROUP_LIST` group-type byte (cm:Group.h:60-64), stored as `game_group.group_type`.
    pub const fn wire(self) -> u8 {
        match self {
            Self::Party => 0,
            Self::Raid => 1,
        }
    }

    /// `None` for a byte no vanilla group type uses.
    pub const fn from_wire(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Party),
            1 => Some(Self::Raid),
            _ => None,
        }
    }

    /// A stored `group_type`. Only [`Self::wire`] writes the column, so the Party fallback is
    /// unreachable. It is the safe reading because it keeps the smaller member cap.
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

/// A member's Subgroup and Assistant flag, kept as the `SMSG_GROUP_LIST` member-flags byte
/// `subgroup | 0x80 if assistant` (cm:Group.cpp:685, 695). Stored as `game_group_member.raid_slot`.
/// A Party member holds the default slot: Subgroup 0, no Assistant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RaidSlot(u8);

impl RaidSlot {
    const ASSISTANT: u8 = 0x80;

    /// `None` for a Subgroup outside 0 to 7.
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

    /// The slot a member joining a Raid takes: the first Subgroup with fewer than
    /// [`SUBGROUP_SIZE`] members, without the Assistant flag (cm:Group.cpp:817-838). `current` is
    /// every current member's slot. `None` when every Subgroup is full.
    pub fn for_raid_joiner(current: impl IntoIterator<Item = Self>) -> Option<Self> {
        let mut sizes = [0usize; RAID_SUBGROUPS as usize];
        for slot in current {
            sizes[usize::from(slot.subgroup())] += 1;
        }
        (0..RAID_SUBGROUPS)
            .find(|&subgroup| sizes[usize::from(subgroup)] < SUBGROUP_SIZE)
            .and_then(|subgroup| Self::new(subgroup, false))
    }

    /// `None` for a byte with a bit set outside the Subgroup and the Assistant flag.
    pub const fn from_wire(byte: u8) -> Option<Self> {
        Self::new(byte & !Self::ASSISTANT, byte & Self::ASSISTANT != 0)
    }

    /// A stored `raid_slot`. Only [`Self::wire`] writes the column, so the fallback to Subgroup 0
    /// without the Assistant flag is unreachable.
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

    /// The same Subgroup with the Assistant flag set or cleared.
    pub const fn with_assistant(self, assistant: bool) -> Self {
        Self(if assistant {
            self.0 | Self::ASSISTANT
        } else {
            self.0 & !Self::ASSISTANT
        })
    }

    /// The result of moving this Raid Slot's member to `destination` Subgroup
    /// (cm:GroupHandler.cpp:492-525, cm:Group.cpp:1203-1251), shared by the Module and the Gateway
    /// Fake so cmangos's capacity-and-destination rule lives in exactly one place. `destination_size`
    /// counts the OTHER members already in `destination` — the mover excluded, since moving into the
    /// Subgroup you already hold can never be blocked by your own presence there.
    ///
    /// `Err(InvalidSubgroup)` for a Subgroup of 8 or more. `Ok(None)` is the same-Subgroup no-op
    /// (cm:Group.cpp:1234), which succeeds regardless of `destination_size`. `Err(SubgroupFull)` when
    /// `destination_size` already holds [`SUBGROUP_SIZE`] members. Otherwise `Ok(Some(slot))` is the
    /// mover's new Raid Slot, its Assistant bit kept.
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

    /// This Raid Slot and `other` after a Swap Subgroup between them (cm:GroupHandler.cpp:901-944).
    /// `None` when both already sit in the same Subgroup — cmangos still answers that as success
    /// (cm:GroupHandler.cpp:939), it just has nothing to swap. One atomic swap can never overfill a
    /// Subgroup, so unlike [`moved_to_subgroup`](Self::moved_to_subgroup) this needs no capacity
    /// argument.
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

/// Group-event kinds (`game_group_event.kind`), what SMSG the gateway relays. Every producer shares
/// this one byte space:
///
/// - 0-3: membership, below.
/// - 4-8: loot rolls and money shares, `crate::loot_roll::event_kind`.
/// - 9: retired. It was party chat, now a Realm Chat Line, and is never reused.
/// - 10-11: quest sharing, `crate::quest::share_event_kind`.
/// - 12-19: unassigned, left for the Realm-core chat seam.
/// - 20: the leader announcement, below.
/// - 21-26: Group Broadcasts, below.
/// - 27-30: reserved for meeting stones.
pub mod event_kind {
    /// You are invited (`other_*` = the inviter) → `SMSG_GROUP_INVITE`.
    pub const INVITE: u8 = 0;
    /// Your group's roster changed → the gateway sends `SMSG_GROUP_LIST` from the row's payload
    /// (payload-carry: the roster is serialized in the SAME transaction as the membership change).
    pub const LIST: u8 = 1;
    /// Your invite was declined (`other_*` = the decliner) → `SMSG_GROUP_DECLINE`.
    pub const DECLINE: u8 = 2;
    /// You are no longer in a group (left / kicked / disbanded) → `SMSG_GROUP_DESTROYED`.
    pub const DESTROYED: u8 = 3;
    // --- work-item 187 (group loot methods) / work-item 221 (money-loot split) — the roll/master-
    // loot/money-share relay REUSES this same per-recipient event table + relay (see
    // `crate::loot_roll`'s module doc for why: identical shape — one recipient, a kind byte, a small
    // payload — so a parallel new table would only add binding-checklist ceremony for zero
    // behavioral gain). Kinds/payload grammar for these five live in `crate::loot_roll`, not here
    // (loot is a distinct concern from group membership) — listed here too ONLY as the reserved
    // kind-byte range so a future group-event kind can never collide with a loot-roll kind sharing
    // the same table.
    /// Reserved range start for `crate::loot_roll::event_kind` — kinds `4..=8` are loot-roll/money-
    /// share kinds relayed through `game_group_event`, not group-membership kinds. The module doc
    /// above lists every range on this table.
    pub const LOOT_ROLL_RESERVED_START: u8 = 4;
    // Kind 9 was party chat, now a Realm Chat Line. It is retired and never reused.
    /// The Group has a new leader (`other_guid`) → `SMSG_GROUP_SET_LEADER` with that leader's name.
    /// Every member receives it before the LIST that names the new leader (cm:Group.cpp:464-470,
    /// 498-501).
    pub const SET_LEADER: u8 = 20;
    /// A Ready Check started (`other_guid` = the starter) → the empty `MSG_RAID_READY_CHECK`.
    pub const READY_CHECK: u8 = 21;
    /// A member answered the Ready Check (`other_guid` = the answerer, payload
    /// [`super::encode_ready_check_answer`]) → `MSG_RAID_READY_CHECK` with the state, to the leader.
    pub const READY_CHECK_ANSWER: u8 = 22;
    /// One Target Icon changed (payload [`super::TargetIcon::encode`]) → the partial
    /// `MSG_RAID_TARGET_UPDATE`.
    pub const TARGET_ICON_UPDATE: u8 = 23;
    /// Every Target Icon the Group holds (payload [`super::encode_target_icons`]) → the full
    /// `MSG_RAID_TARGET_UPDATE`, to the member who asked.
    pub const TARGET_ICON_LIST: u8 = 24;
    /// A member pinged the minimap (`other_guid` = the pinger, payload
    /// [`super::encode_minimap_ping`]) → `MSG_MINIMAP_PING`.
    pub const MINIMAP_PING: u8 = 25;
    /// A `/roll` result (`other_guid` = the roller, payload [`super::encode_random_roll`]) →
    /// `MSG_RANDOM_ROLL`.
    pub const RANDOM_ROLL: u8 = 26;
}

/// The REALM-CORE party ops: the `op` byte of the single operator-gated `realm_group_op` reducer the
/// gateway drives realm-wide membership with.
///
/// One reducer with an op byte, rather than one reducer per op, keeps one generated binding and one
/// signature for every Gateway and durable-test caller. The ARGUMENT SHAPE is pinned here, in the
/// one file both crates import, so a renumber or an argument-slot change is a compile-visible edit
/// on both sides rather than a silent mis-dispatch.
///
/// Argument slots (`realm_group_op(op, actor_guid, target_guid, arg_a, arg_b, arg_c)`), per op:
/// - [`INVITE`] / [`UNINVITE`] — `target_guid` is the invitee/kicked member; the rest unused.
/// - [`ACCEPT`] / [`DECLINE`] — `actor_guid` alone; every other slot unused. `ACCEPT` keeps
///   `arg_a`/`arg_b` free for meeting stones, which will send class and race there.
/// - [`LEAVE`] — `actor_guid` leaves; `arg_a` = a [`super::leave_cause`] value.
/// - [`LOOT_METHOD`] — `arg_a` = loot setting, `target_guid` = the master looter, `arg_b` = the
///   quality threshold. (That is `CMSG_LOOT_METHOD`'s own field order, kept so the gateway hands the
///   three values straight through.)
/// - [`RAID_CONVERT`] — `actor_guid` alone.
/// - [`SET_LEADER`] — `target_guid` is the new leader.
/// - [`SET_ASSISTANT`] — `target_guid` is the member, `arg_a` is 1 to promote and 0 to demote.
/// - [`CHANGE_SUBGROUP`] — `target_guid` is the member to move, `arg_a` is the destination Subgroup.
/// - [`SWAP_SUBGROUP`] — `target_guid` is one member, `arg_c` the other.
/// - [`READY_CHECK_START`] — `actor_guid` alone.
/// - [`READY_CHECK_ANSWER`] — `arg_a` = the client's answer state.
/// - [`TARGET_ICON`] — `arg_a` = the Target Icon, or [`super::TARGET_ICON_LIST_REQUEST`];
///   `target_guid` = the marked unit, 0 to clear the icon.
/// - [`MINIMAP_PING`] — `target_guid` = `x.to_bits()`, `arg_c` = `y.to_bits()`, so the floats
///   cross unchanged.
/// - [`RANDOM_ROLL`] — `target_guid` = the minimum, `arg_c` = the maximum.
///
/// `arg_c` is a `u64` for an op that needs a second guid or a wide value: the subgroup swap's
/// second member, the minimap ping's `y` and the roll's maximum. Every other op sends 0 there.
pub mod realm_op {
    /// `CMSG_GROUP_INVITE` — `actor_guid`, ungrouped, the leader or an Assistant, invites
    /// `target_guid`.
    pub const INVITE: u8 = 0;
    /// `CMSG_GROUP_ACCEPT` — `actor_guid` accepts its pending invite.
    pub const ACCEPT: u8 = 1;
    /// `CMSG_GROUP_DECLINE` — `actor_guid` declines its pending invite.
    pub const DECLINE: u8 = 2;
    /// `CMSG_GROUP_DISBAND` (the client's "Leave Party") — `actor_guid` leaves its group.
    pub const LEAVE: u8 = 3;
    /// `CMSG_GROUP_UNINVITE` and `CMSG_GROUP_UNINVITE_GUID` — the leader or an Assistant,
    /// `actor_guid`, kicks `target_guid`.
    pub const UNINVITE: u8 = 4;
    /// `CMSG_LOOT_METHOD` — leader `actor_guid` sets the party's loot rules.
    pub const LOOT_METHOD: u8 = 5;
    /// `CMSG_GROUP_RAID_CONVERT` — leader `actor_guid` converts its Party to a Raid.
    pub const RAID_CONVERT: u8 = 6;
    /// `CMSG_GROUP_SET_LEADER` — leader `actor_guid` passes the lead to `target_guid`.
    pub const SET_LEADER: u8 = 7;
    /// `CMSG_GROUP_ASSISTANT_LEADER` — Raid leader `actor_guid` promotes or demotes an Assistant.
    pub const SET_ASSISTANT: u8 = 8;
    /// `CMSG_GROUP_CHANGE_SUB_GROUP` — the leader or an Assistant moves `target_guid` to Subgroup
    /// `arg_a`.
    pub const CHANGE_SUBGROUP: u8 = 9;
    /// `CMSG_GROUP_SWAP_SUB_GROUP` — the leader or an Assistant swaps the Subgroups of `target_guid`
    /// and `arg_c`.
    pub const SWAP_SUBGROUP: u8 = 10;
    /// `MSG_RAID_READY_CHECK` without a body — the leader or an Assistant starts a Ready Check.
    pub const READY_CHECK_START: u8 = 11;
    /// `MSG_RAID_READY_CHECK` with a body — `actor_guid` answers the Ready Check.
    pub const READY_CHECK_ANSWER: u8 = 12;
    /// `MSG_RAID_TARGET_UPDATE` — `actor_guid` sets a Target Icon or asks for the full list.
    pub const TARGET_ICON: u8 = 13;
    /// `MSG_MINIMAP_PING` — `actor_guid` pings a point on the minimap.
    pub const MINIMAP_PING: u8 = 14;
    /// `MSG_RANDOM_ROLL` — `actor_guid` rolls a random number.
    pub const RANDOM_ROLL: u8 = 15;
}

/// Why a [`realm_op::LEAVE`] runs, sent in `arg_a`.
pub mod leave_cause {
    /// The Character leaves, or the Gateway leaves for a session-less Character.
    pub const LEFT: u8 = 0;
    /// The Character was deleted. The party authority also drops every Target Icon on it, because
    /// the delete sweep on the Character's World Shard cannot reach Realm-core's icon rows.
    pub const CHARACTER_DELETED: u8 = 1;
}

/// The group op one `game_bot_invite_intent` row asks the Gateway to run.
///
/// A Package writes that row for a Character with no Session, so there is no client behind it and no
/// `ctx.sender()` to resolve; the Gateway turns the byte into the matching [`realm_op`] against the
/// party authority. Two values rather than a reuse of [`realm_op`]: those are what a CLIENT may
/// ask for, and a Package may only ask for these two. [`INVITE`] is `0` because the column was
/// END-appended to a table that had only ever carried invites, so a pre-existing row reads correctly
/// under the migration's own default.
pub mod bot_op {
    /// `inviter_guid` invites `target_guid` into a party.
    pub const INVITE: u8 = 0;
    /// `inviter_guid` leaves its party. `target_guid` is unused.
    pub const LEAVE: u8 = 1;
}

/// Durable companion-command queues are partitioned into this many fair lanes on every source
/// Module. Gateway reads at most one head from each lane per dispatch turn.
pub const COMMAND_DISPATCH_LANES: u8 = 8;

/// A terminal target receipt and its source response remain recoverable for this long after the
/// command's admission deadline.
pub const COMMAND_RESULT_WINDOW_MICROS: i64 = 30_000_000;

/// Why the Module refused a party Durable Request. The tag is the whole reducer error text, so
/// neither tier matches on human prose. Most variants become a `PartyResult` the client renders;
/// [`GroupRefusal::IntentAlreadyClaimed`] instead tells a losing Gateway callback to stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupRefusal {
    /// The acting Character is temporarily unable to perform a party action.
    ActorUnavailable,
    /// A Character may not invite itself.
    InviteSelf,
    /// No Character row holds a guid the op named.
    NoSuchPlayer,
    /// The invited Character has no live entity.
    TargetOffline,
    /// The invited Character is already in a party.
    AlreadyInGroup,
    /// The party is at its member cap.
    GroupFull,
    /// The actor is in a Group but may not run the op there: it does not lead the Group, or is not
    /// an Assistant where one may act. Also the answer when an Assistant names the leader for
    /// removal (cm:GroupHandler.cpp:274-276).
    NotLeader,
    /// The actor is in no party.
    NotInGroup,
    /// The named target is in no party, or in a different one.
    TargetNotInGroup,
    /// Accept or decline ran with no invite standing.
    NoPendingInvite,
    /// The inviter is gone, or no longer leads its Group or assists in it.
    InviterUnavailable,
    /// The actor tried to kick itself; leaving is the op for that.
    KickSelf,
    /// The loot method, threshold, or master looter is not a legal setting.
    InvalidLootRules,
    /// Another Gateway already claimed this bot invite intent.
    IntentAlreadyClaimed,
    /// The Package has suppressed session-less actions for this Character.
    ActionSuppressed,
    /// The invited Character belongs to the other team. Vanilla's default refuses a party across
    /// factions (cm:GroupHandler.cpp:80). The Gateway applies this Gate realm-wide.
    WrongFaction,
    /// The op needs a Raid, and the actor's Group is a Party.
    NotRaid,
    /// A leader named itself as the new leader or as an Assistant (vm:GroupHandler.cpp:311, 554).
    TargetIsSelf,
    /// A Change Subgroup or Swap Subgroup destination named a Subgroup outside 0 to 7.
    InvalidSubgroup,
    /// A Change Subgroup destination already holds [`SUBGROUP_SIZE`] members.
    SubgroupFull,
    /// A Target Icon index outside 0 to 7 that is not the list request (cm:Group.cpp:585-586).
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

/// A Group holds 8 Target Icons, numbered 0 to 7 (cm:Group.h:43).
pub const TARGET_ICON_COUNT: u8 = 8;

/// The `MSG_RAID_TARGET_UPDATE` index that asks for every Target Icon instead of setting one
/// (cm:GroupHandler.cpp:457-459).
pub const TARGET_ICON_LIST_REQUEST: u8 = 0xFF;

/// One Target Icon on one unit. `target_guid` 0 means the icon is clear.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetIcon {
    pub icon: u8,
    pub target_guid: u64,
}

impl TargetIcon {
    /// `icon,target_guid`, the `TARGET_ICON_UPDATE` payload.
    pub fn encode(self) -> String {
        format!("{},{}", self.icon, self.target_guid)
    }

    /// `None` for a malformed payload or an icon outside 0 to 7.
    pub fn decode(payload: &str) -> Option<Self> {
        let (icon, target_guid) = payload.split_once(',')?;
        let icon = icon.parse().ok().filter(|&icon| icon < TARGET_ICON_COUNT)?;
        Some(Self {
            icon,
            target_guid: target_guid.parse().ok()?,
        })
    }
}

/// `icon,target_guid;...`, the `TARGET_ICON_LIST` payload and a Party roster's icon segment. An
/// empty list is an empty string.
pub fn encode_target_icons(icons: &[TargetIcon]) -> String {
    icons
        .iter()
        .map(|icon| icon.encode())
        .collect::<Vec<_>>()
        .join(";")
}

/// `None` for a malformed entry, a clear icon, or two entries sharing an icon or a unit: a Group
/// holds each icon once and a unit carries one icon at most.
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

/// The `READY_CHECK_ANSWER` payload: the client's state byte in decimal.
pub fn encode_ready_check_answer(state: u8) -> String {
    state.to_string()
}

pub fn decode_ready_check_answer(payload: &str) -> Option<u8> {
    payload.parse().ok()
}

/// `x,y` as `f32` bit patterns, the `MINIMAP_PING` payload. Bits rather than decimals, so every
/// float the client sent reaches the other members unchanged.
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

/// `min,max,result`, the `RANDOM_ROLL` payload.
pub fn encode_random_roll(min: u32, max: u32, result: u32) -> String {
    format!("{min},{max},{result}")
}

/// `None` for a malformed payload or a result outside its range.
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

/// One roster as a LIST event carries it: what `SMSG_GROUP_LIST` shows, except presence.
///
/// The Module writes this in the same transaction as the change it reports, so the roster and the
/// loot rules always agree. Presence is not in it: on Realm-core the Module has no live entities to
/// read, so the Gateway reads presence from the shard caches when it renders the list. Names are
/// blank on Realm-core for the same reason and the Gateway fills them the same way.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RosterPayload {
    pub leader: u64,
    pub loot_method: u8,
    pub loot_threshold: u8,
    pub master_looter_guid: u64,
    pub kind: GroupKind,
    /// Every member in join order, the recipient included.
    pub members: Vec<RosterMember>,
    /// A Party's Target Icons, sent again right after the list. In a Party the client clears its
    /// marks on every `SMSG_GROUP_LIST`, and vmangos resends them after the list
    /// (vm:Group.cpp:1343-1360). A Raid keeps its marks, so a Raid's roster carries none.
    pub target_icons: Vec<TargetIcon>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RosterMember {
    pub guid: u64,
    pub name: String,
    pub slot: RaidSlot,
}

impl RosterPayload {
    /// `leader,loot_method,loot_threshold,master_looter_guid,kind|guid,name,slot;...`, with `kind`
    /// and `slot` as wire bytes, then `|` and [`encode_target_icons`] when there are Target Icons.
    /// The encoder owns the grammar, so it also strips the delimiters from names. Character
    /// creation admits only letters today, but that rule lives elsewhere.
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

    /// `None` for a malformed or extra field, an unknown kind, an invalid Raid Slot, no members,
    /// more members than a Raid holds, or a malformed icon segment. The Gateway fails closed rather
    /// than render a corrupt roster.
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

    /// cm:Group.h:40-43: a party of 5, a raid of 40 in 8 Subgroups of 5.
    #[test]
    fn a_raid_is_eight_subgroups_of_five_and_a_party_caps_at_five() {
        assert_eq!(GROUP_MAX_MEMBERS, 5);
        assert_eq!(RAID_MAX_MEMBERS, 40);
        assert_eq!(RAID_SUBGROUPS, 8);
        assert_eq!(SUBGROUP_SIZE, 5);
        assert_eq!(GroupKind::Party.member_cap(), 5);
        assert_eq!(GroupKind::Raid.member_cap(), 40);
    }

    /// The group-type byte is `GROUP_FLAG_NORMAL = 0`, `GROUP_FLAG_RAID = 1` (cm:Group.h:60-64). It
    /// is also the stored column, whose END-appended default 0 must read as a Party.
    #[test]
    fn the_group_kind_byte_is_the_vanilla_group_type() {
        assert_eq!(GroupKind::default(), GroupKind::Party);
        assert_eq!(GroupKind::Party.wire(), 0);
        assert_eq!(GroupKind::Raid.wire(), 1);
        assert_eq!(GroupKind::from_wire(0), Some(GroupKind::Party));
        assert_eq!(GroupKind::from_wire(1), Some(GroupKind::Raid));
        assert_eq!(GroupKind::from_wire(2), None);
    }

    /// The flags byte is `group | (assistant ? 0x80 : 0)` (cm:Group.cpp:685, 695). The stored
    /// column's default 0 is Subgroup 0 without the Assistant flag.
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

    /// The slots of a Raid whose Subgroup `n` holds `sizes[n]` members, the first of each an
    /// Assistant.
    fn raid_of(sizes: [u8; 8]) -> Vec<RaidSlot> {
        (0u8..8)
            .flat_map(|subgroup| {
                (0..sizes[usize::from(subgroup)])
                    .map(move |index| RaidSlot::new(subgroup, index == 0).unwrap())
            })
            .collect()
    }

    /// cm:Group.cpp:817-838: the first Subgroup below 5 members, else no room. An Assistant
    /// counts like any member, and the joiner is never an Assistant.
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

    /// AC 3, 4: a full destination Subgroup refuses, an out-of-range one refuses, and the
    /// Assistant bit survives an accepted move.
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

    /// cm:Group.cpp:1234: moving to your own Subgroup succeeds and changes nothing, even when that
    /// Subgroup is already full — the mover is presumably one of the five already counted there.
    #[test]
    fn a_subgroup_move_to_the_same_subgroup_is_a_no_op() {
        let mover = RaidSlot::new(3, false).unwrap();
        assert_eq!(mover.moved_to_subgroup(3, 5), Ok(None));
    }

    /// AC 6: a swap between two full Subgroups needs no capacity Gate — the pair trade places
    /// atomically, so neither Subgroup ever holds six members mid-swap.
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

    /// AC 7: cm:GroupHandler.cpp:939 — two members already in one Subgroup swap to a no-op.
    #[test]
    fn a_subgroup_swap_within_one_subgroup_is_a_no_op() {
        let first = RaidSlot::new(4, true).unwrap();
        let second = RaidSlot::new(4, false).unwrap();
        assert_eq!(first.swapped_with(second), None);
    }

    /// Pinned against the grammar, not recomputed: kind `1` in the head, and each member's flags
    /// byte in decimal, `0x81` = 129 for an Assistant in Subgroup 1.
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
            "x,3,2,0,0|1,A,0",   // non-numeric leader
            "2,3,2,0,0|",        // no members
            "2|1,A,0",           // no loot rules
            "2,3,2,0|1,A,1",     // the old grammar, which had no kind
            "2,3,2,0,2|1,A,0",   // unknown kind
            "2,3,2,0,0,9|1,A,0", // extra head field
            "2,3,2,0,0|x,Bob,0", // non-numeric guid
            "2,3,2,0,0|5,Bob",   // no slot
            "2,3,2,0,0|5",       // no name, no slot
            "2,3,2,0,0|5,Bob,8", // ninth Subgroup
            "2,3,2,0,0|5,Bob,maybe",
            "2,3,2,0,0|5,Bob,0,1",         // extra member field
            "2,3,2,0,0|5,Bob,0|7",         // an icon with no unit
            "2,3,2,0,0|5,Bob,0|8,99",      // a ninth icon
            "2,3,2,0,0|5,Bob,0|7,99;7,98", // one icon twice
            "2,3,2,0,0|5,Bob,0|7,99;6,99", // one unit with two icons
            "2,3,2,0,0|5,Bob,0|7,0",       // a clear icon in a held list
            "2,3,2,0,0|5,Bob,0|7,99|6,98", // a fourth segment
        ] {
            assert_eq!(RosterPayload::decode(payload), None, "{payload:?}");
        }
    }

    // ---- Realm-core party ops (issue #22, group slice) ----

    /// The intent op byte is a MIGRATION value as well as a wire one: `op` was END-appended to
    /// `game_bot_invite_intent` with a `0` default, so every row written before the column existed
    /// reads as an INVITE. Renumbering would turn those rows into leaves.
    #[test]
    fn bot_group_intent_op_codes_are_stable_and_distinct() {
        assert_eq!(bot_op::INVITE, 0, "the END-appended column defaults to 0");
        assert_eq!(bot_op::LEAVE, 1);
        assert_ne!(bot_op::INVITE, bot_op::LEAVE);
    }

    /// The op byte is a WIRE value: the gateway sends it, the module dispatches on it, and the two
    /// are deployed separately. A renumber that only one side learns about silently runs the wrong
    /// op — an ACCEPT arriving as a LEAVE — so the numbering is pinned, not merely defined.
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

    /// Promoting sets only the Assistant bit and demoting clears only it; the Subgroup stays.
    #[test]
    fn promoting_or_demoting_keeps_the_subgroup() {
        let plain = RaidSlot::new(3, false).unwrap();
        assert_eq!(plain.with_assistant(true).wire(), 0x83);
        assert_eq!(plain.with_assistant(true).with_assistant(false), plain);
        assert_eq!(plain.with_assistant(false), plain);
    }

    /// The leader announcement is kind 20 and collides with no kind another producer writes to
    /// the same table.
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

    // ---- Group Broadcasts ----

    /// Every producer of `game_group_event` shares one kind byte, so a Group Broadcast kind that
    /// collided with a membership, loot or quest-share kind would render the wrong packet.
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

    /// `arg_a` defaults to 0 in every older LEAVE caller, so 0 must stay a plain leave.
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

    /// cm:Group.h:43 holds 8 icons; 0xFF asks for the list (cm:GroupHandler.cpp:457).
    #[test]
    fn a_group_holds_eight_target_icons_and_0xff_asks_for_the_list() {
        assert_eq!(TARGET_ICON_COUNT, 8);
        assert_eq!(TARGET_ICON_LIST_REQUEST, 0xFF);
    }

    /// Pinned against the grammar: the icon segment follows the members only when there is one.
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

    /// Decimal text would round a float; bit patterns carry every value the client sent.
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
