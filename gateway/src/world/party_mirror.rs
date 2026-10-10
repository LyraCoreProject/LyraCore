//! The Roster Revision Relay: push a party's Realm-core roster to each World Shard whose mirror
//! holds an older Roster Revision or none, whatever changed it. `party::run` pushes only the
//! actor's parties, and a Stone Add changes parties the actor is not in.
//!
//! Row callbacks only mark a party dirty, and one worker thread drains the set, so no reducer call
//! runs on an SDK callback thread. A push that fails every attempt is logged and dropped; the party
//! waits for its next revision or the reconnect pass.

use super::party;
use super::WorldStore;
use crate::world::ShardRoutingStore;
use std::collections::BTreeSet;
use std::sync::{Arc, Condvar, Mutex, PoisonError};

#[derive(Default)]
pub(crate) struct RosterRevisionRelay {
    dirty: Mutex<BTreeSet<u64>>,
    wake: Condvar,
}

impl RosterRevisionRelay {
    /// Start the worker thread. `store` is any Coordinator handle.
    pub(crate) fn spawn<St: Send + ShardRoutingStore + 'static>(
        store: St,
    ) -> std::io::Result<Arc<Self>> {
        let relay = Arc::new(Self::default());
        let worker = relay.clone();
        std::thread::Builder::new()
            .name("roster-revision-relay".into())
            .spawn(move || loop {
                worker.wait_for_dirty();
                // A panic in one pass must not end the relay for the life of the process.
                let pass = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    worker.push_dirty(&store);
                }));
                if pass.is_err() {
                    log::error!(
                        "party: a Roster Revision Relay pass panicked; its parties wait for their \
                         next revision or the reconnect pass"
                    );
                }
            })?;
        Ok(relay)
    }

    /// Safe on an SDK callback thread: one short lock, no calls.
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

    /// Push every dirty party to the World Shards whose mirror is stale. A party marked during the
    /// pass waits for the next one.
    pub(crate) fn push_dirty<St: ShardRoutingStore + ?Sized>(&self, store: &St) {
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

fn push_party<St: ShardRoutingStore + ?Sized>(
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

/// A newer mirrored revision means this Gateway's Realm-core cache lags, and the shard would refuse
/// the older push anyway.
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
