//! Member Stats: the Store seam that finds a group member on any World Shard, the Relay tick that
//! decides what one viewer is sent, and the `CMSG_REQUEST_PARTY_MEMBER_STATS` answer.
//!
//! A member inside the viewer's AOI is already created on the client, which draws its frame from
//! the object fields. Member Stats go only to the others (cm:Group.cpp:718-733).

use super::super::*;
use crate::codec::{
    build_member_offline, build_member_stats, stats_delta, GroupUpdateMask, MemberStats,
    MemberStatsPacket,
};
use std::collections::HashMap;

/// Where one group member is, as the Gateway's Coordinator caches see it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MemberPresence {
    /// A live `game_world_entity` row on some World Shard.
    Live(MemberStats),
    /// No live entity, but the member is between two places: a pending Transfer, or the loading
    /// screen of a map change. Reporting it offline would make the frame flicker.
    InTransit,
    Offline,
}

pub(crate) trait MemberStatsStore: Send + Sync {
    /// Every other member of `self_guid`'s group, from the party authority's cache: Realm-core on
    /// a sharded Realm, the only database otherwise. Empty when `self_guid` has no group.
    fn group_mates(&self, self_guid: u64) -> Result<Vec<u64>>;

    /// The member's presence. `Live` reads the first World Shard cache that holds the entity.
    fn member_presence(&self, guid: u64) -> Result<MemberPresence>;
}

impl MemberStatsStore for crate::stdb::Coordinator {
    fn group_mates(&self, self_guid: u64) -> Result<Vec<u64>> {
        crate::stdb::Coordinator::group_mates(self, self_guid)
    }

    fn member_presence(&self, guid: u64) -> Result<MemberPresence> {
        crate::stdb::Coordinator::member_presence(self, guid)
    }
}

/// What the Relay last sent one viewer about one group mate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MemberSnapshot {
    Offline,
    Live(MemberStats),
}

/// One Relay tick for one viewer: the `SMSG_PARTY_MEMBER_STATS` packets it needs, in group order.
///
/// `snapshots` is the viewer's record of what it holds. It changes only when the tick succeeds, so
/// a failed read never leaves a record of a packet that was not sent. A mate created on the client
/// loses its record, so the tick after it leaves the AOI sends every field again, because the
/// client's party cache may be stale by then.
pub(crate) fn member_stats_tick<St: MemberStatsStore + ?Sized>(
    store: &St,
    self_guid: u64,
    is_created: impl Fn(u64) -> bool,
    snapshots: &mut HashMap<u64, MemberSnapshot>,
) -> Result<Vec<Outbound>> {
    let mates = store.group_mates(self_guid)?;
    let mut presences = Vec::with_capacity(mates.len());
    for guid in mates {
        let presence = if is_created(guid) {
            None
        } else {
            Some(store.member_presence(guid)?)
        };
        presences.push((guid, presence));
    }

    snapshots.retain(|guid, _| presences.iter().any(|(mate, _)| mate == guid));
    let mut outbound = Vec::new();
    for (guid, presence) in presences {
        let (opcode, body) = match presence {
            None => {
                snapshots.remove(&guid);
                continue;
            }
            Some(MemberPresence::InTransit) => continue,
            Some(MemberPresence::Offline) => {
                if snapshots.insert(guid, MemberSnapshot::Offline) == Some(MemberSnapshot::Offline)
                {
                    continue;
                }
                build_member_offline(MemberStatsPacket::Changed, guid)
            }
            Some(MemberPresence::Live(stats)) => {
                let previous = match snapshots.insert(guid, MemberSnapshot::Live(stats)) {
                    Some(MemberSnapshot::Live(previous)) => Some(previous),
                    Some(MemberSnapshot::Offline) | None => None,
                };
                let mask = stats_delta(previous.as_ref(), &stats);
                if mask.is_empty() {
                    continue;
                }
                build_member_stats(MemberStatsPacket::Changed, guid, mask, &stats)
            }
        };
        outbound.push(Outbound::Raw { opcode, body });
    }
    Ok(outbound)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MemberStatsPlayer {
    pub(crate) self_guid: Option<u64>,
}

pub(crate) enum MemberStatsOutcome {
    Handled { outbound: Vec<Outbound> },
    PassThrough(ClientOpcodeMessage),
}

/// Answer `CMSG_REQUEST_PARTY_MEMBER_STATS` with one `SMSG_PARTY_MEMBER_STATS_FULL`.
///
/// Only the requester's group mates get live values. Any other guid gets the offline answer
/// (cm:GroupHandler.cpp:764-771), as vmangos does (vm:GroupHandler.cpp:782). cmangos answers any
/// guid, which would show a stranger's zone and position to anyone who asks. A Store failure is
/// an `Err` and ends the session, like every other Durable Read failure.
pub(crate) fn dispatch_member_stats<St: MemberStatsStore + ?Sized>(
    store: &St,
    player: MemberStatsPlayer,
    msg: ClientOpcodeMessage,
) -> Result<MemberStatsOutcome> {
    let (ClientOpcodeMessage::CMSG_REQUEST_PARTY_MEMBER_STATS(request), Some(self_guid)) =
        (&msg, player.self_guid)
    else {
        return Ok(MemberStatsOutcome::PassThrough(msg));
    };
    let guid = request.guid.guid();
    let presence = if store.group_mates(self_guid)?.contains(&guid) {
        store.member_presence(guid)?
    } else {
        MemberPresence::Offline
    };
    let (opcode, body) = match presence {
        MemberPresence::Live(stats) => build_member_stats(
            MemberStatsPacket::Full,
            guid,
            GroupUpdateMask::MEMBER,
            &stats,
        ),
        MemberPresence::InTransit | MemberPresence::Offline => {
            build_member_offline(MemberStatsPacket::Full, guid)
        }
    };
    Ok(MemberStatsOutcome::Handled {
        outbound: vec![Outbound::Raw { opcode, body }],
    })
}
