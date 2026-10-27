use std::{fs, sync::Arc};

use anyhow::{ensure, Context};
use spacetimedb_durability::{Durability as _, DurableOffset};
use spacetimedb_runtime::{sync::watch, Handle};
use spacetimedb_snapshot::{SnapshotRepository, CURRENT_MODULE_ABI_VERSION};
use spacetimedb_table::page_pool::PagePool;

use crate::{relational_db::LocalDurability, util::asyncify};

pub(crate) async fn run(
    mut snapshots: watch::Receiver<Option<u64>>,
    repo: Arc<SnapshotRepository>,
    replica_id: u64,
    durability: LocalDurability,
    runtime: Handle,
) {
    snapshots.borrow_and_update();
    let mut durable = durability.durable_tx_offset();
    while let Some(offset) = next_durable_snapshot(&mut snapshots, &mut durable).await {
        let repo = repo.clone();
        let durability = durability.clone();
        let result = asyncify(&runtime, move || {
            let Some(first_required) = retain_snapshots(&repo, replica_id, offset)? else {
                return Ok(0);
            };
            durability
                .prune_before(first_required)
                .map_err(anyhow::Error::from)
        })
        .await;
        match result {
            Ok(removed) if removed > 0 => {
                log::info!("Development reload retained two snapshots; removed {removed} commit log segments");
            }
            Ok(_) => {}
            Err(error) => log::warn!("Development reload history cleanup pass failed: {error:#}"),
        }
    }
}

async fn next_durable_snapshot(
    snapshots: &mut watch::Receiver<Option<u64>>,
    durable: &mut DurableOffset,
) -> Option<u64> {
    loop {
        snapshots.changed().await.ok()?;
        let offset = *snapshots.borrow_and_update();
        if let Some(offset) = offset {
            durable.wait_for(offset).await.ok()?;
            return Some(offset);
        }
    }
}

fn retain_snapshots(
    repo: &SnapshotRepository,
    replica_id: u64,
    durable: u64,
) -> anyhow::Result<Option<u64>> {
    let mut offsets: Vec<_> = repo
        .all_snapshots()?
        .filter(|offset| *offset <= durable)
        .collect();
    offsets.sort_unstable();
    if offsets.len() < 2 {
        return Ok(None);
    }
    let retained = &offsets[offsets.len() - 2..];
    let pool = PagePool::new(None);
    for &offset in retained {
        let snapshot = repo
            .read_snapshot(offset, &pool)
            .with_context(|| format!("validating snapshot {offset}"))?;
        ensure!(
            snapshot.database_identity == repo.database_identity()
                && snapshot.replica_id == replica_id
                && snapshot.tx_offset == offset
                && snapshot.module_abi_version == CURRENT_MODULE_ABI_VERSION,
            "snapshot {offset} does not match this database"
        );
        fs::File::open(repo.snapshot_dir_path(offset))?.sync_all()?;
    }
    let oldest_retained = retained[0];
    let first_required = oldest_retained
        .checked_add(1)
        .context("snapshot offset overflow")?;
    let directory = repo.snapshot_dir_path(oldest_retained);
    let parent = directory
        .0
        .parent()
        .context("snapshot directory has no parent")?;
    // Persist snapshot names and removed lockfiles before deleting any recovery history.
    fs::File::open(parent)?.sync_all()?;
    for offset in offsets
        .into_iter()
        .take_while(|offset| *offset < oldest_retained)
    {
        fs::remove_dir_all(repo.snapshot_dir_path(offset))?;
    }
    fs::File::open(parent)?.sync_all()?;
    Ok(Some(first_required))
}

#[cfg(test)]
mod tests {
    use super::*;
    use spacetimedb_commitlog::{Commitlog, Options};
    use spacetimedb_lib::Identity;
    use spacetimedb_paths::server::{CommitLogDir, SnapshotsPath};
    use spacetimedb_paths::FromPathUnchecked;
    use spacetimedb_table::blob_store::{BlobStore, HashMapBlobStore};

    struct Fixture {
        _directory: tempfile::TempDir,
        snapshots: SnapshotsPath,
        repo: SnapshotRepository,
        log_path: CommitLogDir,
        log: Commitlog<()>,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let snapshots = SnapshotsPath::from_path_unchecked(directory.path().join("snapshots"));
            snapshots.create().unwrap();
            let repo = SnapshotRepository::open(snapshots.clone(), Identity::ZERO, 1).unwrap();
            for offset in [9, 19, 29] {
                let mut blobs = HashMapBlobStore::default();
                blobs.insert_blob(&vec![offset as u8; 2048]);
                repo.create_snapshot(std::iter::empty(), &blobs, offset)
                    .unwrap()
                    .sync_all()
                    .unwrap();
            }
            let log_path = CommitLogDir::from_path_unchecked(directory.path().join("clog"));
            log_path.create().unwrap();
            let log = Commitlog::open(
                log_path.clone(),
                Options {
                    max_segment_size: 1,
                    ..Options::default()
                },
                None,
            )
            .unwrap();
            for offset in (0..48).step_by(3) {
                log.commit((offset..offset + 3).map(|offset| (offset, ())))
                    .unwrap();
                log.flush_and_sync().unwrap();
            }
            Self {
                _directory: directory,
                snapshots,
                repo,
                log_path,
                log,
            }
        }

        fn offsets(&self) -> Vec<u64> {
            let mut offsets: Vec<_> = self.repo.all_snapshots().unwrap().collect();
            offsets.sort_unstable();
            offsets
        }
    }

    #[test]
    fn retained_snapshots_and_log_reopen_with_the_fallback_replay_range() {
        let fixture = Fixture::new();
        let first_required = retain_snapshots(&fixture.repo, 1, 47).unwrap().unwrap();
        assert_eq!(first_required, 20);
        assert_eq!(fixture.offsets(), [19, 29]);
        assert_eq!(fixture.log.prune_before(first_required).unwrap(), 6);
        assert_eq!(fixture.log.min_committed_offset(), Some(18));
        let paths = fixture.log.existing_segment_offsets().unwrap();
        assert_eq!(paths, (18..=48).step_by(3).collect::<Vec<_>>());
        assert_eq!(fixture.log.prune_before(first_required).unwrap(), 0);
        drop(fixture.log);
        let reopened = Commitlog::<()>::open(fixture.log_path, Options::default(), None).unwrap();
        assert_eq!(reopened.min_committed_offset(), Some(18));
        assert_eq!(reopened.max_committed_offset(), Some(47));
        let replayed: Vec<_> = reopened
            .commits_from(20)
            .flat_map(|commit| commit.unwrap().tx_range())
            .filter(|offset| *offset >= 20)
            .collect();
        assert_eq!(replayed, (20..48).collect::<Vec<_>>());
        assert_eq!(reopened.prune_before(u64::MAX).unwrap(), 9);
        assert_eq!(reopened.existing_segment_offsets().unwrap(), [45]);
    }

    #[tokio::test]
    async fn snapshot_notification_waits_for_its_durable_offset() {
        let (snapshots, mut notifications) = watch::channel(None);
        let (persisted, offsets) = watch::channel(Some(18));
        let mut durable = DurableOffset::from(offsets);
        let next = next_durable_snapshot(&mut notifications, &mut durable);
        futures::pin_mut!(next);
        snapshots.send(Some(29)).unwrap();
        assert!(futures::poll!(&mut next).is_pending());
        persisted.send(Some(28)).unwrap();
        assert!(futures::poll!(&mut next).is_pending());
        persisted.send(Some(29)).unwrap();
        assert_eq!(next.await, Some(29));
    }

    #[tokio::test]
    async fn durability_shutdown_cannot_authorize_snapshot_cleanup() {
        let (snapshots, mut notifications) = watch::channel(None);
        let (persisted, offsets) = watch::channel(Some(18));
        let mut durable = DurableOffset::from(offsets);
        let next = next_durable_snapshot(&mut notifications, &mut durable);
        futures::pin_mut!(next);
        snapshots.send(Some(29)).unwrap();
        assert!(futures::poll!(&mut next).is_pending());
        drop(persisted);
        assert_eq!(next.await, None);
    }

    #[test]
    fn snapshots_ahead_of_durable_history_do_not_allow_cleanup() {
        let fixture = Fixture::new();
        assert_eq!(retain_snapshots(&fixture.repo, 1, 18).unwrap(), None);
        assert_eq!(fixture.offsets(), [9, 19, 29]);
        assert_eq!(retain_snapshots(&fixture.repo, 1, 19).unwrap(), Some(10));
        assert_eq!(fixture.offsets(), [9, 19, 29]);
    }

    #[test]
    fn deletion_failure_stops_before_removing_later_segments() {
        let fixture = Fixture::new();
        let blocked = fixture.log_path.segment(3);
        let saved = fs::read(&blocked).unwrap();
        fs::remove_file(&blocked).unwrap();
        fs::create_dir(&blocked).unwrap();
        assert!(fixture.log.prune_before(20).is_err());
        assert_eq!(fixture.log.min_committed_offset(), Some(3));
        assert!(fixture.log_path.segment(6).0.exists());
        fs::remove_dir(&blocked).unwrap();
        fs::write(&blocked, saved).unwrap();
        assert_eq!(fixture.log.prune_before(20).unwrap(), 5);
        assert_eq!(fixture.log.min_committed_offset(), Some(18));
    }

    #[test]
    fn damaged_snapshot_preserves_all_history() {
        let fixture = Fixture::new();
        let latest = fixture.repo.snapshot_dir_path(29);
        fs::write(latest.snapshot_file(29), b"damaged snapshot").unwrap();
        assert!(retain_snapshots(&fixture.repo, 1, 47).is_err());
        assert_eq!(fixture.offsets(), [9, 19, 29]);
    }

    #[test]
    fn a_snapshot_for_another_replica_cannot_authorize_cleanup() {
        let fixture = Fixture::new();
        let foreign =
            SnapshotRepository::open(fixture.snapshots.clone(), Identity::ZERO, 2).unwrap();
        foreign
            .create_snapshot(std::iter::empty(), &HashMapBlobStore::default(), 39)
            .unwrap()
            .sync_all()
            .unwrap();
        let error = retain_snapshots(&fixture.repo, 1, 47).unwrap_err();
        assert!(error.to_string().contains("does not match this database"));
        assert_eq!(fixture.offsets(), [9, 19, 29, 39]);
    }
}
