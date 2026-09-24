//! Member Stats: the Store seam that finds a group member on any World Shard, the Relay tick that
//! decides what one viewer is sent, and the `CMSG_REQUEST_PARTY_MEMBER_STATS` answer.
//!
//! A member inside the viewer's AOI is already created on the client, which draws its frame from
//! the object fields. Member Stats go only to the others (cm:Group.cpp:718-733).

use super::super::*;
use crate::codec::{
    build_member_stats, build_member_status, full_update_mask, member_status, stats_delta,
    MemberStats, MemberStatsPacket,
};
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// Where one group member is, as the Gateway's Coordinator caches see it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MemberPresence {
    /// A live `game_world_entity` row on some World Shard. Boxed: `MemberStats` carries a 32-slot
    /// and a 16-slot aura array plus its pet's own copies, so an unboxed variant would triple the
    /// size of every `MemberPresence`, most of which are `InTransit` or `Offline`.
    Live(Box<MemberStats>),
    /// No live entity, but the member is between two places: a pending Transfer, or the loading
    /// screen of a map change. Reporting it offline would make the frame flicker.
    InTransit,
    Offline,
}

pub(crate) trait MemberStatsStore: Send + Sync {
    /// Every other member of `self_guid`'s group, from the party authority's cache: Realm-core on
    /// a sharded Realm, the only database otherwise. Empty when `self_guid` has no group.
    fn group_mates(&self, self_guid: u64) -> Result<Vec<u64>>;

    /// The member's presence, projected from its Realm Presence
    /// ([`crate::world::presence::of`]): a live entity is `Live`, `Whereabouts::InTransit` is
    /// `InTransit`, and `Whereabouts::Offline` or no Realm Presence at all is `Offline`.
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MemberSnapshot {
    Offline,
    /// Boxed for the same reason as [`MemberPresence::Live`].
    Live(Box<MemberStats>),
}

/// What one World Session's client holds of each group mate's Member Stats.
///
/// The Relay and the session share it, and only jobs on the session's writer change it, so it
/// follows the order in which the client reads packets. A packet that sets a frame some other way
/// (`SMSG_GROUP_LIST`, a FULL answer) forgets what it overwrote, and the next tick sends every
/// field of that member again.
#[derive(Clone, Default)]
pub(crate) struct MemberStatsRecord(Arc<Mutex<HashMap<u64, MemberSnapshot>>>);

impl MemberStatsRecord {
    pub(crate) fn lock(&self) -> MutexGuard<'_, HashMap<u64, MemberSnapshot>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn forget(&self, guid: u64) {
        self.lock().remove(&guid);
    }

    pub(crate) fn forget_all(&self) {
        self.lock().clear();
    }
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
                build_member_status(MemberStatsPacket::Changed, guid, member_status::OFFLINE)
            }
            Some(MemberPresence::Live(stats)) => {
                let previous = match snapshots.insert(guid, MemberSnapshot::Live(stats.clone())) {
                    Some(MemberSnapshot::Live(previous)) => Some(previous),
                    Some(MemberSnapshot::Offline) | None => None,
                };
                let mask = stats_delta(previous.as_deref(), &stats);
                if mask.is_empty() {
                    continue;
                }
                build_member_stats(
                    MemberStatsPacket::Changed,
                    guid,
                    mask,
                    previous.as_deref(),
                    &stats,
                )
            }
        };
        outbound.push(Outbound::Raw { opcode, body });
    }
    Ok(outbound)
}

#[derive(Clone, Copy, Default)]
pub(crate) struct MemberStatsPlayer<'a> {
    pub(crate) self_guid: Option<u64>,
    /// `None` only for a Store with no Relay, such as the in-memory test Store.
    pub(crate) record: Option<&'a MemberStatsRecord>,
}

pub(crate) enum MemberStatsOutcome {
    Handled { outbound: Vec<Outbound> },
    PassThrough(ClientOpcodeMessage),
}

/// Answer `CMSG_REQUEST_PARTY_MEMBER_STATS` with one `SMSG_PARTY_MEMBER_STATS_FULL`.
///
/// The answer is a writer job that also forgets the member in the viewer's record, so the next
/// Relay tick sends every field on top of it. A failed read logs and sends nothing, because a
/// party frame is never worth a World Session.
pub(crate) fn dispatch_member_stats<St: MemberStatsStore + ?Sized>(
    store: &St,
    player: MemberStatsPlayer<'_>,
    msg: ClientOpcodeMessage,
) -> MemberStatsOutcome {
    let (ClientOpcodeMessage::CMSG_REQUEST_PARTY_MEMBER_STATS(request), Some(self_guid)) =
        (&msg, player.self_guid)
    else {
        return MemberStatsOutcome::PassThrough(msg);
    };
    let guid = request.guid.guid();
    let (opcode, body) = match full_answer(store, self_guid, guid) {
        Ok(answer) => answer,
        Err(error) => {
            log::warn!("member stats: no answer for {guid} to {self_guid}: {error:#}");
            return MemberStatsOutcome::Handled {
                outbound: Vec::new(),
            };
        }
    };
    let record = player.record.cloned();
    let answer = move || {
        if let Some(record) = record {
            record.forget(guid);
        }
        vec![Outbound::Raw { opcode, body }]
    };
    MemberStatsOutcome::Handled {
        outbound: vec![Outbound::Job(Box::new(answer))],
    }
}

/// Only the requester's group mates get their state. Any other guid gets the offline answer
/// (cm:GroupHandler.cpp:764-771), as in vmangos (vm:GroupHandler.cpp:782). cmangos answers any
/// guid, which would show a stranger's zone and position to anyone who asks.
///
/// A mate between two places answers online with `ZONE_OUT`. cmangos finds a teleporting player
/// too (`FindPlayer(guid, false)`, cm:GroupHandler.cpp:764) and flags it `ZONE_OUT`
/// (cm:Group.cpp:54-55). Its entity is gone here, so the answer carries the status alone and the
/// client keeps the other fields it has.
fn full_answer<St: MemberStatsStore + ?Sized>(
    store: &St,
    self_guid: u64,
    guid: u64,
) -> Result<(u16, Vec<u8>)> {
    let packet = MemberStatsPacket::Full;
    if !store.group_mates(self_guid)?.contains(&guid) {
        return Ok(build_member_status(packet, guid, member_status::OFFLINE));
    }
    Ok(match store.member_presence(guid)? {
        MemberPresence::Live(stats) => {
            let mask = full_update_mask(&stats);
            build_member_stats(packet, guid, mask, None, &stats)
        }
        MemberPresence::InTransit => build_member_status(
            packet,
            guid,
            member_status::ONLINE | member_status::ZONE_OUT,
        ),
        MemberPresence::Offline => build_member_status(packet, guid, member_status::OFFLINE),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // The live-entity/in-transit/absence-gated-offline decision itself is pinned in
    // `world::presence_tests`, against `presence::of` — this file no longer runs its own
    // discovery, so it no longer needs its own Store Fake for one.

    #[test]
    fn forgetting_one_member_keeps_the_others() {
        let record = MemberStatsRecord::default();
        record.lock().insert(1, MemberSnapshot::Offline);
        record.lock().insert(2, MemberSnapshot::Offline);
        record.forget(1);
        assert_eq!(record.lock().keys().copied().collect::<Vec<_>>(), [2]);
        record.forget_all();
        assert!(record.lock().is_empty());
    }
}
