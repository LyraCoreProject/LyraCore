//! `Coordinator`'s [`ShardRoutingStore`] adapter.

use anyhow::Result;

use crate::stdb::Coordinator;
use crate::world::{ShardRoutingStore, WorldStore};

impl ShardRoutingStore for Coordinator {
    fn shard_name(&self) -> &str {
        // Inherent method (see the module note): this is a view, not recursion.
        self.shard_name()
    }

    /// The production world-entry resolver: find where the character actually LIVES, resolve who
    /// should own it, and run the escrowed transfer when those differ. Single-shard → the
    /// `is_sharded` short-circuit, so the unconfigured gateway does not even read a row.
    ///
    /// The HOLDER lookup below is `realm_core::locate_home_shard` — the realm-core
    /// index consulted FIRST, the shard scan only on a miss, self-heal write-back on either path.
    fn settle_home_shard(
        &self,
        character_guid: u64,
    ) -> Result<Option<std::sync::Arc<dyn WorldStore>>> {
        if !self.is_sharded() {
            return Ok(None);
        }
        // The shard that currently holds the durable row (or the in-flight escrow). Realm-core's
        // character→shard index is consulted first; the scan (`Coordinator::all_shards`) is the
        // fallback, not the only path.
        let Some(holder) = crate::realm_core::locate_home_shard(self, character_guid) else {
            // Unknown character: leave the session where it is. The
            // module's own ownership check is what refuses the login.
            return Ok(None);
        };
        // The ESCROW's destination outranks the durable row's location: `settle_transfer` RESUMES
        // an existing escrow against the destination the escrow was opened for, so an owner
        // resolved from anything else would hand `run_transfer` a `dst` that is not the database the
        // blob is already filed against — importing the character into a third shard and orphaning
        // the fenced copy. `run_transfer` cannot catch that for itself: a `&dyn WorldStore` cannot be
        // asked which `(map, instance)` it owns, so the obligation is here, at the resolution site.
        // With no escrow (the fresh-transfer path) this is the durable row, exactly as before — the
        // same fields `character_destination` builds the fresh plan from.
        let escrow = holder.escrow_row(character_guid);
        let (map_id, instance_id) = escrow
            .as_ref()
            .map(|r| (r.dest_map_id, r.dest_instance_id))
            .or_else(|| holder.character_location(character_guid))
            .ok_or_else(|| anyhow::anyhow!("character {character_guid} vanished mid-routing"))?;
        // OWNER RESOLUTION. The shard map answers, in DATABASE NAMES resolved through
        // `shard_for`/`instance_shard_for`, so "the answer names the shard I already am" decodes to
        // `None` in both, and `self` is the handle that means "stay put". The NAME never depends on
        // which handle asked (`shard_for` and `world_shards` both read the shared `ShardSet`), so
        // this replaces the older "resolve from the default handle" without reintroducing
        // `realm_shard()`, which realm-core deleted.
        //
        // The instance-pool stickiness applies ONLY when there is no escrow in flight. With an
        // escrow the destination is already fixed by the row (see the paragraph above): being
        // sticky about the holder there would resolve an owner that differs from the database the
        // blob is filed against, which is the third-shard hazard this comment block exists to
        // prevent. Without one, the holder is the only durable evidence of which pool member a
        // live run is on, and it is what stops a pool resize from forking a party.
        let owner = if escrow.is_none() {
            self.instance_shard_for(map_id, instance_id, holder.shard_name())
        } else {
            self.shard_for(map_id, instance_id)
        }
        .unwrap_or_else(|| self.clone());
        // The three facts that decide whether a resume is DRIVEN or silently skipped. Nothing logged
        // them, so every diagnosis of a stranded copy (twice, live) had to infer the holder from what
        // did not happen — and `settle_transfer`'s `holder == owner` branch is silent by design.
        log::info!(
            "settle {character_guid}: holder={} owner={} escrow={} ({map_id}/{instance_id})",
            holder.shard_name(),
            owner.shard_name(),
            escrow.is_some()
        );
        crate::world::transfer::settle_transfer(&holder, &owner, character_guid)?;
        Ok(Some(
            std::sync::Arc::new(owner) as std::sync::Arc<dyn WorldStore>
        ))
    }

    /// `instance_shard_for`, not `shard_for`: this handle is the character's HOLDER, and the
    /// stickiness rule that keeps a live dungeon run on its pool member is written against exactly
    /// that pair. `settle_home_shard` above resolves a fresh (escrow-free) hop the same way.
    fn shard_for_location(
        &self,
        map_id: u32,
        instance_id: u64,
    ) -> Option<std::sync::Arc<dyn WorldStore>> {
        let shard = self.instance_shard_for(map_id, instance_id, Coordinator::shard_name(self))?;
        Some(std::sync::Arc::new(shard) as std::sync::Arc<dyn WorldStore>)
    }

    fn bind_shard_session(&self, account_id: u64, session_key: &[u8; 40]) -> Result<()> {
        // `bound_identity` DERIVES this shard's identity for the account
        // (`synthetic_owner_identity`); `establish_session` then binds it to the shadow account
        // row that `import_character_blob` created. On the realm shard this is the same binding
        // the logon tier already wrote, re-asserted — idempotent, and cheap next to a login.
        let identity = self.bound_identity(account_id)?;
        self.establish_session(account_id, session_key, identity)
    }

    /// The realm-core handle, but ONLY on a multi-database gateway.
    ///
    /// The `is_sharded()` short-circuit is the whole of "unset env vars ⇒ today's gateway" for the
    /// realm-wide party plane: with one database `realm_core()` would answer THIS handle, and routing
    /// party ops through the operator-gated realm reducers against the same database would be a
    /// different code path, a different gate, and a different set of writes for no gain. Answering
    /// `None` here sends `world::party` down the arm that calls exactly the reducers it called
    /// before realm-core existed.
    ///
    /// Configured-but-unreachable realm-core answers `None` as well (`realm_core()` is `Err`), so a
    /// realm-core outage degrades party ops to shard-local rather than failing them. That is a
    /// deliberate split from the AUTH paths, which fail CLOSED: a stale auth cache lets someone in
    /// with a revoked credential, while a shard-local party op is only ever a smaller party.
    fn realm_store(&self) -> Option<std::sync::Arc<dyn WorldStore>> {
        if !self.is_sharded() {
            return None;
        }
        self.realm_core()
            .ok()
            .map(|rc| std::sync::Arc::new(rc) as std::sync::Arc<dyn WorldStore>)
    }

    fn party_cleanup_realm(&self) -> Result<Option<std::sync::Arc<dyn WorldStore>>> {
        if !self.is_sharded() {
            return Ok(None);
        }
        self.realm_core()
            .map(|realm| Some(std::sync::Arc::new(realm) as std::sync::Arc<dyn WorldStore>))
    }

    fn party_command_realm(&self) -> Result<Option<std::sync::Arc<dyn WorldStore>>> {
        if !self.is_sharded() {
            return Ok(None);
        }
        self.realm_core()
            .map(|realm| Some(std::sync::Arc::new(realm) as std::sync::Arc<dyn WorldStore>))
    }

    fn transfer_realm(&self) -> Result<Option<std::sync::Arc<dyn WorldStore>>> {
        if !self.is_sharded() {
            return Ok(None);
        }
        self.realm_core()
            .map(|realm| Some(std::sync::Arc::new(realm) as std::sync::Arc<dyn WorldStore>))
    }

    /// Every connected WORLD shard (realm-core excluded by `ShardMap::shards`, as always) — the
    /// mirror fan-out set. Empty when unsharded, so the push costs a single-database gateway nothing.
    fn world_stores(&self) -> Vec<std::sync::Arc<dyn WorldStore>> {
        if !self.is_sharded() {
            return Vec::new();
        }
        self.all_shards()
            .into_iter()
            .map(|c| std::sync::Arc::new(c) as std::sync::Arc<dyn WorldStore>)
            .collect()
    }

    fn party_command_worlds(&self) -> Result<Vec<std::sync::Arc<dyn WorldStore>>> {
        if !self.is_sharded() {
            return Ok(Vec::new());
        }
        Ok(self
            .configured_world_shards()?
            .into_iter()
            .map(|coordinator| std::sync::Arc::new(coordinator) as std::sync::Arc<dyn WorldStore>)
            .collect())
    }
}
