//! The Member Stats Relay. Every [`MEMBER_STATS_INTERVAL`] its own thread queues one job per World
//! Session. Each job runs [`member_stats_tick`] on that session's writer, so no shard pump does the
//! work and the packets stay in order with the session's other Relays.
//!
//! The Relay reads cached rows only. Each Gateway process serves its own sessions from its own
//! caches, so several Gateways need no coordination.

use std::sync::Arc;
use std::time::Duration;

use super::connection::Coordinator;
use super::world_view::{self, Viewer, WorldView};
use crate::world::{member_stats_tick, MemberStatsStore, Outbound};

/// The player heartbeat that sends cmangos group updates (cm:Player.cpp:1609-1613, 5000 ms at
/// cm:Object.h:861).
const MEMBER_STATS_INTERVAL: Duration = Duration::from_secs(5);

pub(super) fn spawn_member_stats_relay(coordinator: Coordinator) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("stdb-member-stats-relay".into())
        .spawn(move || {
            let view = coordinator.world_view();
            loop {
                std::thread::sleep(MEMBER_STATS_INTERVAL);
                queue_member_stats(&view, &coordinator);
            }
        })
        .expect("spawn member stats relay")
}

/// Queue one Member Stats job on every registered viewer's writer.
fn queue_member_stats<St: MemberStatsStore + Clone + 'static>(view: &WorldView, store: &St) {
    for viewer in view.all_viewers() {
        let store = store.clone();
        world_view::enqueue(viewer, move |viewer| viewer_member_stats(&store, &viewer));
    }
}

/// A failed read skips this viewer until the next tick. Its record stays as it was.
fn viewer_member_stats<St: MemberStatsStore + ?Sized>(
    store: &St,
    viewer: &Arc<Viewer>,
) -> Vec<Outbound> {
    let mut snapshots = viewer.member_stats.lock();
    let is_created = |guid| {
        viewer
            .created
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(&guid)
    };
    match member_stats_tick(store, viewer.self_guid, is_created, &mut snapshots) {
        Ok(outbound) => outbound,
        Err(error) => {
            log::debug!(
                "member stats: skipped viewer {} this tick: {error:#}",
                viewer.self_guid
            );
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::MemberStats;
    use crate::stdb::aoi::ViewerGates;
    use crate::stdb::world_index::CellKey;
    use crate::stdb::world_view::{ExplorationReplay, MotionPending};
    use crate::world::{MemberPresence, SessionTx};
    use anyhow::Result;
    use std::collections::{HashMap, HashSet};
    use std::sync::Mutex;

    const VIEWER: u64 = 10;
    const MATE: u64 = 11;

    /// One party of two, the mate live everywhere.
    #[derive(Clone)]
    struct TwoMemberParty;

    impl MemberStatsStore for TwoMemberParty {
        fn group_mates(&self, self_guid: u64) -> Result<Vec<u64>> {
            Ok(if self_guid == VIEWER {
                vec![MATE]
            } else {
                Vec::new()
            })
        }

        fn member_presence(&self, _guid: u64) -> Result<MemberPresence> {
            Ok(MemberPresence::Live(Box::new(MemberStats {
                status: 1,
                current_health: 100,
                ..MemberStats::default()
            })))
        }
    }

    fn viewer(tx: SessionTx) -> Arc<Viewer> {
        Arc::new(Viewer {
            active: std::sync::atomic::AtomicBool::new(true),
            session: 1,
            self_guid: VIEWER,
            bound_identity: spacetimedb_sdk::Identity::ZERO,
            map_id: 0,
            instance_id: 0,
            zone_id: 0.into(),
            tx,
            created: Arc::new(Mutex::new(HashSet::from([VIEWER]))),
            gates: Arc::new(ViewerGates::default()),
            skill_slots: Arc::new(Mutex::new((HashMap::new(), 0))),
            explored: Mutex::new(ExplorationReplay::default()),
            motion_pending: Arc::new(MotionPending::default()),
            member_stats: Default::default(),
            ignored: Mutex::default(),
            friends: Mutex::default(),
            team: lyracore_shared::faction::TEAM_ALLIANCE,
        })
    }

    #[test]
    fn each_viewer_gets_one_job_on_its_own_writer() {
        let view = WorldView::new(true);
        let (tx, rx) = SessionTx::with_depth(0);
        let viewer = viewer(tx);
        view.add_viewer_on_shard(viewer.clone(), CellKey::at(0, 0, 0, 0), 0);

        queue_member_stats(&view, &TwoMemberParty);

        let Ok(Outbound::Job(job)) = rx.try_recv() else {
            panic!("the Relay must queue work on the viewer's writer, not send packets");
        };
        assert!(rx.try_recv().is_err(), "one job per viewer per tick");
        assert!(
            viewer.member_stats.lock().is_empty(),
            "the Relay thread does no per-viewer work itself"
        );
        let packets = job();
        assert!(matches!(
            packets.as_slice(),
            [Outbound::Raw { opcode: 0x007E, .. }]
        ));
        assert!(viewer.member_stats.lock().contains_key(&MATE));
    }

    #[test]
    fn a_mate_created_on_the_client_gets_nothing_from_the_job() {
        let view = WorldView::new(true);
        let (tx, rx) = SessionTx::with_depth(0);
        let viewer = viewer(tx);
        viewer.created.lock().unwrap().insert(MATE);
        view.add_viewer_on_shard(viewer, CellKey::at(0, 0, 0, 0), 0);

        queue_member_stats(&view, &TwoMemberParty);

        let Ok(Outbound::Job(job)) = rx.try_recv() else {
            panic!("expected a writer job");
        };
        assert!(job().is_empty());
    }
}
