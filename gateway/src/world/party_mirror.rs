//! The Roster Revision Relay: whatever changes a party on Realm-core, push that party's roster to
//! each World Shard whose mirror holds an older Roster Revision or none.
//!
//! `party::run` pushes only the acting Character's before and after parties. Realm-core can also
//! change parties no actor of the op belongs to, so the mirror follows the revision instead. The
//! Realm-core row callbacks only mark a party dirty; one worker thread per Gateway process drains
//! the dirty set, so several revisions of one party before the worker runs cost one push, and no
//! reducer call runs on an SDK callback thread.
//!
//! A push that still fails after [`party::sync_group_mirrors_required`]'s attempts is logged and
//! dropped. The party waits for its next revision, or for the reconciliation pass a Realm-core
//! reconnect starts.

use super::party;
use super::WorldStore;
use std::collections::BTreeSet;
use std::sync::{Arc, Condvar, Mutex, PoisonError};

/// The dirty party ids and the worker that pushes them.
#[derive(Default)]
pub(crate) struct RosterRevisionRelay {
    dirty: Mutex<BTreeSet<u64>>,
    wake: Condvar,
}

impl RosterRevisionRelay {
    /// Start the worker thread. `store` is any Coordinator handle: it names Realm-core and every
    /// World Shard.
    pub(crate) fn spawn<St: WorldStore + 'static>(store: St) -> std::io::Result<Arc<Self>> {
        let relay = Arc::new(Self::default());
        let worker = relay.clone();
        std::thread::Builder::new()
            .name("roster-revision-relay".into())
            .spawn(move || loop {
                worker.wait_for_dirty();
                worker.push_dirty(&store);
            })?;
        Ok(relay)
    }

    /// Note that Realm-core moved this party's Roster Revision. Safe on an SDK callback thread:
    /// it takes one short lock and calls nothing.
    pub(crate) fn mark_dirty(&self, group_id: u64) {
        self.dirty
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(group_id);
        self.wake.notify_one();
    }

    fn wait_for_dirty(&self) {
        let mut dirty = self.dirty.lock().unwrap_or_else(PoisonError::into_inner);
        while dirty.is_empty() {
            dirty = self
                .wake
                .wait(dirty)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Take every dirty party and push it to the World Shards whose mirror is stale. A party marked
    /// again while this runs stays for the next pass.
    pub(crate) fn push_dirty<St: WorldStore + ?Sized>(&self, store: &St) {
        let parties =
            std::mem::take(&mut *self.dirty.lock().unwrap_or_else(PoisonError::into_inner));
        if parties.is_empty() {
            return;
        }
        let realm = match store.party_cleanup_realm() {
            Ok(Some(realm)) => realm,
            Ok(None) => return,
            Err(error) => {
                log::warn!(
                    "party: the Roster Revision Relay cannot reach Realm-core ({error:#}); {} \
                     party mirror(s) wait for the reconnect pass",
                    parties.len()
                );
                return;
            }
        };
        let shards = store.world_stores();
        for group_id in parties {
            push_party(store, realm.as_ref(), &shards, group_id);
        }
    }
}

fn push_party<St: WorldStore + ?Sized>(
    store: &St,
    realm: &dyn WorldStore,
    shards: &[Arc<dyn WorldStore>],
    group_id: u64,
) {
    let realm_revision = match realm.held_roster_revision(group_id) {
        Ok(Some(revision)) => revision,
        Ok(None) => return,
        Err(error) => {
            log::warn!(
                "party: could not read Realm-core's Roster Revision for group {group_id} \
                 ({error:#}); its mirrors wait for the next revision"
            );
            return;
        }
    };
    for shard in shards {
        // A failed read cannot prove the mirror current, and a repeated push is idempotent.
        let mirrored = shard.held_roster_revision(group_id).unwrap_or(None);
        if !mirror_is_stale(realm_revision, mirrored) {
            continue;
        }
        if let Err(error) = party::sync_group_mirrors_required(
            store,
            realm,
            group_id,
            None,
            std::slice::from_ref(shard),
        ) {
            log::warn!(
                "party: group {group_id} mirror on {} stays stale until its next Roster Revision \
                 or the reconnect pass ({error:#})",
                shard.shard_name()
            );
        }
    }
}

/// A World Shard mirror needs Realm-core's roster when it holds no row for the party or an older
/// Roster Revision. A newer one means this Gateway's Realm-core cache lags; pushing it would be
/// refused as older anyway.
fn mirror_is_stale(realm_revision: u64, mirrored: Option<u64>) -> bool {
    mirrored.is_none_or(|mirrored| mirrored < realm_revision)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mirror_with_no_row_for_the_party_is_stale() {
        assert!(mirror_is_stale(1, None));
    }

    #[test]
    fn an_older_mirror_is_stale_and_an_equal_or_newer_one_is_current() {
        assert!(mirror_is_stale(4, Some(3)));
        assert!(!mirror_is_stale(4, Some(4)));
        assert!(!mirror_is_stale(4, Some(5)));
    }
}
