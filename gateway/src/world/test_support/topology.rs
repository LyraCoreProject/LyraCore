use super::*;

#[derive(Default)]
pub(crate) struct TopologyState {
    /// Multi-shard routing: the database this handle stands for. `""` (derive-Default) is the
    /// single-shard world every other test runs in, where nothing routes.
    pub(crate) shard: String,
    /// The handle `home_shard()` hands back — the character's home shard. `None` (default) = "you
    /// are already on the right shard", i.e. the single-entry shard map / pre-sharding behavior.
    pub(crate) home: Option<std::sync::Arc<WorldFake>>,
    /// Home-shard reassignment: when set, every `home_shard()` resolution AFTER the first
    /// answers THIS shard instead of `home` — the mock's stand-in for a routing change landing
    /// between two logins (a shard-map edit, or the realm-core index re-homing a character). `None`
    /// (default): every resolution answers `home`, byte-identical to before this field existed.
    pub(crate) home_after_flip: Option<std::sync::Arc<WorldFake>>,
    /// The one location this handle routes elsewhere, and where to: `(map_id, instance_id, shard)`.
    /// `None` (derive-Default) = "this handle serves every location", the single-database answer
    /// `shard_for_location` gives. Keyed by location on purpose — a caller that asks about the
    /// wrong place gets `None` and the crossing silently becomes a no-op, which is what the tests
    /// must be able to tell apart from a crossing that ran.
    pub(crate) location_shard: Option<(u32, u64, std::sync::Arc<WorldFake>)>,
    /// How many times `home_shard()` has been asked — drives `home_after_flip`, and is itself the
    /// assertion that routing is resolved ONCE PER WORLD ENTRY and never mid-session. SHARED
    /// between a store and the handles it routes to (like `calls`), so a re-resolution asked of the
    /// *pinned* handle — which is what a mid-session re-route would actually look like, since
    /// `route_home` asks whichever handle the session currently holds — is counted too.
    pub(crate) home_shard_calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    /// SHARED between a store and its `home` handle: `(shard, call)` for every instrumented
    /// player-scoped call, in order. The routing test asserts nothing lands on the wrong database.
    pub(crate) calls: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
    /// The handle `realm_store()` hands back — the database that owns party
    /// membership realm-wide. `None` (derive-Default) is the SINGLE-DATABASE gateway, which is what
    /// every other test in this file is, and it is what routes every party op back onto the
    /// player-facing reducers below.
    pub(crate) realm: Option<std::sync::Arc<WorldFake>>,
    /// The connected WORLD shards `world_stores()` fans the roster mirror out to (and the
    /// cross-shard name/presence lookups walk). Empty = single database. Behind a `Mutex` only so
    /// the topology can be wired up AFTER every handle exists — production reads the shared
    /// `ShardSet`, which has the same shape and the same "includes this handle" membership.
    pub(crate) peers: std::sync::Mutex<Vec<std::sync::Arc<WorldFake>>>,
    /// When set, the configured World Shard set is incomplete or unhealthy.
    pub(crate) world_shard_set_error: Option<String>,
    /// When set, deleted Character cleanup cannot reach Realm-core.
    pub(crate) party_cleanup_realm_error: Option<String>,
    pub(crate) party_command_realm_error: Option<String>,
    pub(crate) transfer_realm_error: Option<String>,
    /// When set, `settle_home_shard` fails with this message (a transfer that could not be
    /// driven — an unreachable destination shard, a refused import).
    pub(crate) settle_error: Option<String>,
    /// How many `settle_home_shard` calls SUCCEED before `settle_error` starts firing. 0
    /// (derive-Default) = the very first one fails, i.e. the login-time failure the transfer
    /// test drives.
    /// 1 = the login routes fine and the WORLD-PORT's settle is the one that cannot be driven —
    /// the case that hung a real client on its loading screen forever.
    pub(crate) settle_ok_calls: usize,
    /// How many times `settle_home_shard` has been asked (drives `settle_ok_calls`).
    pub(crate) settle_calls: std::sync::atomic::AtomicUsize,
    /// Accounts `bind_shard_session` was called for, per shard.
    pub(crate) bound_sessions: std::sync::Mutex<Vec<u64>>,
    /// Whether this character still had an addressable viewer at each transfer-resolution call.
    pub(crate) viewer_present_at_settle: std::sync::Mutex<Vec<bool>>,
}

impl ShardRoutingStore for WorldFake {
    fn shard_name(&self) -> &str {
        &self.topology.shard
    }

    // The Fake enforces the Module's escrow guards so transfer ordering affects outcomes.

    fn settle_home_shard(
        &self,
        character_guid: u64,
    ) -> Result<Option<std::sync::Arc<dyn WorldStore>>> {
        if let Some(view) = &self.session.relay_view {
            self.topology.viewer_present_at_settle.lock().unwrap().push(
                view.viewer_of_owner(crate::stdb::world_view::OwnerGuid(character_guid))
                    .is_some(),
            );
        }
        if let Some(e) = &self.topology.settle_error {
            let nth = self
                .topology
                .settle_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if nth >= self.topology.settle_ok_calls {
                return Err(anyhow!("{e}"));
            }
        }
        Ok(self.home_shard(character_guid))
    }

    fn bind_shard_session(&self, account_id: u64, _session_key: &[u8; 40]) -> Result<()> {
        self.rec("bind_shard_session");
        self.topology
            .bound_sessions
            .lock()
            .unwrap()
            .push(account_id);
        Ok(())
    }

    fn shard_for_location(
        &self,
        map_id: u32,
        instance_id: u64,
    ) -> Option<std::sync::Arc<dyn WorldStore>> {
        let (m, i, shard) = self.topology.location_shard.as_ref()?;
        (*m == map_id && *i == instance_id).then(|| shard.clone() as std::sync::Arc<dyn WorldStore>)
    }

    fn realm_store(&self) -> Option<std::sync::Arc<dyn WorldStore>> {
        self.topology
            .realm
            .clone()
            .map(|r| r as std::sync::Arc<dyn WorldStore>)
    }

    fn party_cleanup_realm(&self) -> Result<Option<std::sync::Arc<dyn WorldStore>>> {
        if let Some(error) = &self.topology.party_cleanup_realm_error {
            return Err(anyhow!(error.clone()));
        }
        Ok(self.realm_store())
    }

    fn party_command_realm(&self) -> Result<Option<std::sync::Arc<dyn WorldStore>>> {
        if let Some(error) = &self.topology.party_command_realm_error {
            return Err(anyhow!(error.clone()));
        }
        Ok(self.realm_store())
    }

    fn transfer_realm(&self) -> Result<Option<std::sync::Arc<dyn WorldStore>>> {
        if let Some(error) = &self.topology.transfer_realm_error {
            return Err(anyhow!(error.clone()));
        }
        Ok(self.realm_store())
    }

    fn world_stores(&self) -> Vec<std::sync::Arc<dyn WorldStore>> {
        self.topology
            .peers
            .lock()
            .unwrap()
            .iter()
            .map(|p| p.clone() as std::sync::Arc<dyn WorldStore>)
            .collect()
    }

    fn party_command_worlds(&self) -> Result<Vec<std::sync::Arc<dyn WorldStore>>> {
        if let Some(error) = &self.topology.world_shard_set_error {
            return Err(anyhow!(error.clone()));
        }
        Ok(self.world_stores())
    }
}

impl WorldFake {
    /// Record one player-scoped call against THIS handle's shard.
    pub(crate) fn rec(&self, what: &str) {
        self.topology
            .calls
            .lock()
            .unwrap()
            .push((self.topology.shard.clone(), what.to_string()));
    }

    pub(crate) fn home_shard(
        &self,
        _character_guid: u64,
    ) -> Option<std::sync::Arc<dyn WorldStore>> {
        let nth = self
            .topology
            .home_shard_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let resolved = match (&self.topology.home_after_flip, nth) {
            (Some(flipped), n) if n >= 1 => Some(flipped.clone()),
            _ => self.topology.home.clone(),
        };
        resolved.map(|h| h as std::sync::Arc<dyn WorldStore>)
    }
}
