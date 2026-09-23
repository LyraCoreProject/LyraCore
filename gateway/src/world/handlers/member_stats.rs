//! Member Stats: the Store seam that finds a group member on any World Shard, the Relay tick that
//! decides what one viewer is sent, and the `CMSG_REQUEST_PARTY_MEMBER_STATS` answer.
//!
//! A member inside the viewer's AOI is already created on the client, which draws its frame from
//! the object fields. Member Stats go only to the others (cm:Group.cpp:718-733).

use super::super::*;
use crate::codec::{
    build_member_stats, build_member_status, full_update_mask, member_status, stats_delta,
    MemberEntity, MemberStats, MemberStatsPacket,
};
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// Where one group member is, as the Gateway's Coordinator caches see it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MemberPresence {
    /// A live `game_world_entity` row on some World Shard. Boxed: `MemberStats` carries two
    /// 48-slot aura arrays plus its pet's own copies, so an unboxed variant would triple the size
    /// of every `MemberPresence`, most of which are `InTransit` or `Offline`.
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

    /// The member's presence, decided by [`locate_member`].
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

/// One World Shard's cached answers about one Character.
pub(crate) trait MemberShardCache {
    fn member_entity(&self, guid: u64) -> Option<MemberEntity>;

    /// This shard shows the Character between two places. Either its Session is online while its
    /// entity is gone for a map change, or a Transfer Intent names it. A sessionless bot never
    /// has an online Session, so the Transfer Intent is the only sign that a bot is crossing.
    fn member_between_places(&self, guid: u64) -> bool;
}

/// Decide where a group member is. A live entity on any connected World Shard wins. Offline claims
/// absence, so it needs `every_shard`: every configured World Shard, each healthy. An unreadable
/// shard or Realm-core is an `Err`, never Offline.
pub(crate) fn locate_member<S: MemberShardCache>(
    guid: u64,
    connected: &[S],
    realm_transfer_pending: impl FnOnce() -> Result<bool>,
    every_shard: impl FnOnce() -> Result<Vec<S>>,
) -> Result<MemberPresence> {
    if let Some(entity) = connected.iter().find_map(|shard| shard.member_entity(guid)) {
        return Ok(MemberPresence::Live(Box::new(MemberStats::from_entity(
            &entity,
        ))));
    }
    if realm_transfer_pending()? {
        return Ok(MemberPresence::InTransit);
    }
    let between_places = every_shard()?
        .iter()
        .any(|shard| shard.member_between_places(guid));
    Ok(if between_places {
        MemberPresence::InTransit
    } else {
        MemberPresence::Offline
    })
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

    const MATE: u64 = 9;

    /// One World Shard's cache: a live entity, a Character between two places, or neither.
    #[derive(Clone, Default)]
    struct Shard {
        entity: Option<MemberEntity>,
        between_places: bool,
    }

    impl MemberShardCache for Shard {
        fn member_entity(&self, guid: u64) -> Option<MemberEntity> {
            self.entity.clone().filter(|_| guid == MATE)
        }

        fn member_between_places(&self, guid: u64) -> bool {
            self.between_places && guid == MATE
        }
    }

    const LIVE: Shard = Shard {
        entity: Some(MemberEntity {
            health: 100,
            max_health: 100,
            power: 0,
            max_power: 0,
            unit_bytes_0: 0,
            level: 1,
            zone_id: 12,
            x: 0.0,
            y: 0.0,
            dead: false,
            player_flags: 0,
            auras: Vec::new(),
            pet: None,
        }),
        between_places: false,
    };
    const EMPTY: Shard = Shard {
        entity: None,
        between_places: false,
    };
    const CROSSING: Shard = Shard {
        entity: None,
        between_places: true,
    };

    fn locate(
        connected: &[Shard],
        transfer_pending: Result<bool>,
        every_shard: Result<Vec<Shard>>,
    ) -> Result<MemberPresence> {
        locate_member(MATE, connected, || transfer_pending, || every_shard)
    }

    fn unhealthy() -> Result<Vec<Shard>> {
        Err(anyhow!("instances has no healthy Coordinator subscription"))
    }

    #[test]
    fn a_live_entity_on_any_connected_shard_is_live_even_while_another_shard_is_down() {
        let presence = locate(&[EMPTY, LIVE], Err(anyhow!("unread")), unhealthy()).unwrap();
        assert!(matches!(presence, MemberPresence::Live(_)));
    }

    #[test]
    fn a_pending_transfer_on_realm_core_is_in_transit() {
        let presence = locate(&[EMPTY], Ok(true), unhealthy()).unwrap();
        assert_eq!(presence, MemberPresence::InTransit);
    }

    #[test]
    fn a_character_between_places_on_any_shard_is_in_transit() {
        let presence = locate(&[EMPTY, EMPTY], Ok(false), Ok(vec![EMPTY, CROSSING])).unwrap();
        assert_eq!(presence, MemberPresence::InTransit);
    }

    #[test]
    fn offline_needs_every_configured_shard_healthy() {
        assert!(locate(&[EMPTY], Ok(false), unhealthy()).is_err());
        assert!(locate(&[EMPTY], Err(anyhow!("realm-core down")), Ok(vec![EMPTY])).is_err());
        let presence = locate(&[EMPTY], Ok(false), Ok(vec![EMPTY, EMPTY])).unwrap();
        assert_eq!(presence, MemberPresence::Offline);
    }

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
