use super::super::*;

#[derive(Default)]
pub(crate) struct TransferState {
    /// The fake DATABASE this handle talks to, for the cross-database transfer tests. `None`
    /// (the default) leaves every transfer trait method at its "this store does not shard" default,
    /// so every other test in this file is untouched.
    pub(crate) xdb: Option<std::sync::Arc<FakeShardDb>>,
    /// How many `escrowed_transfer` reads answer `None` before the row shows up — the
    /// cross-connection lag between a reducer's reply and the coordinator cache, made dialable.
    pub(crate) escrow_reads_before_visible: std::sync::atomic::AtomicUsize,
    /// The transfer step to fail at, simulating a gateway killed before that step's
    /// transaction committed. `None` = nothing fails.
    pub(crate) kill_at: Option<String>,
    /// The REALM-CORE character→shard index this handle's `publish_shard_index` writes. In
    /// production that write goes to a third database (`realm_core()`); here it is just a map, so a
    /// test can assert the drive published the destination it settled on.
    pub(crate) realm_index: std::sync::Mutex<Vec<(u64, u32, u64)>>,
    /// Realm-core's ordered transfer phase for cross-Shard caller tests.
    pub(crate) realm_partition:
        std::sync::Mutex<Option<crate::world::party::RealmCharacterPartition>>,
    /// When set, `publish_shard_index` fails with this message — an unreachable realm-core.
    pub(crate) publish_error: Option<String>,
}

impl TransferStore for WorldFake {
    fn escrowed_transfer(
        &self,
        character_guid: u64,
    ) -> Option<crate::world::transfer::EscrowedTransfer> {
        let db = self.transfer.xdb.as_ref()?;
        if self
            .transfer
            .escrow_reads_before_visible
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |left| left.checked_sub(1),
            )
            .is_ok()
        {
            return None;
        }
        let out = lk(&db.out_rows);
        out.values()
            .find(|e| e.character_guid == character_guid)
            .map(|e| crate::world::transfer::EscrowedTransfer {
                transfer_id: e.transfer_id,
                character_guid: e.character_guid,
                dest_map_id: e.dest_map_id,
                dest_instance_id: e.dest_instance_id,
                blob: e.blob.clone(),
            })
    }

    fn character_destination(
        &self,
        character_guid: u64,
    ) -> Option<crate::world::transfer::TransferPlan> {
        let c = self.transfer.xdb.as_ref()?.get(character_guid)?;
        Some(crate::world::transfer::TransferPlan {
            transfer_id: crate::world::transfer::transfer_id_for(character_guid),
            character_guid,
            dest_map_id: c.map_id,
            dest_instance_id: c.instance_id,
            dest_x: 0.0,
            dest_y: 0.0,
            dest_z: 0.0,
            dest_o: 0.0,
        })
    }

    /// `plan_begin`: replay on a matching escrow, refuse without a source copy, refuse a second
    /// escrow for the same character, otherwise freeze + serialize.
    ///
    /// The `escrowed_guid` lookup is the OUT-row with the IN-row as a fallback, exactly as
    /// `module/src/transfer/mod.rs`'s `begin_transfer` computes it — and that fallback is load-bearing
    /// now the transfer id IS the character guid: a database holding an unreleased ARRIVAL in-row
    /// for this character answers `BeginPlan::Replay` to a genuine new transfer, i.e. reports
    /// success while freezing nothing. A mock that only looked at `out_rows` could not see it.
    fn begin_transfer(&self, plan: &crate::world::transfer::TransferPlan) -> Result<()> {
        let db = self.xstep("begin_transfer")?;
        let mut out = lk(&db.out_rows);
        let escrowed_guid = out
            .get(&plan.transfer_id)
            .map(|e| e.character_guid)
            .or_else(|| lk(&db.in_rows).get(&plan.transfer_id).copied());
        if let Some(existing) = escrowed_guid {
            if existing != plan.character_guid {
                return Err(anyhow!("transfer id collision"));
            }
            return Ok(()); // BeginPlan::Replay
        }
        let Some(c) = db.get(plan.character_guid) else {
            return Err(anyhow!("no such character: {}", plan.character_guid));
        };
        if out
            .values()
            .any(|e| e.character_guid == plan.character_guid)
        {
            return Err(anyhow!(
                "character {} is already in transit",
                plan.character_guid
            ));
        }
        out.insert(
            plan.transfer_id,
            FakeEscrow {
                transfer_id: plan.transfer_id,
                character_guid: plan.character_guid,
                dest_map_id: plan.dest_map_id,
                dest_instance_id: plan.dest_instance_id,
                blob: fake_blob(
                    plan.character_guid,
                    plan.dest_map_id,
                    plan.dest_instance_id,
                    &c.payload,
                ),
            },
        );
        Ok(())
    }

    /// `import_character_blob`: replay on the in-row PK, refuse to land on a LIVE character,
    /// otherwise materialise the row + its payload at the escrow's destination.
    fn import_character_blob(
        &self,
        transfer_id: u64,
        blob: &[u8],
        source: crate::world::transfer::RealmLocatorPredecessor,
        bot_arrival: Option<&crate::world::transfer::BotTransferIntent>,
    ) -> Result<()> {
        let db = self.xstep("import_character_blob")?;
        let (guid, arriving) = parse_blob(blob);
        if let Some(intent) = bot_arrival {
            if intent.source_module_identity == spacetimedb_sdk::Identity::ZERO
                || intent.id == 0
                || intent.created_micros <= 0
                || source.revision == 0
                || transfer_id != guid
            {
                return Err(anyhow!("bot Transfer arrival identity is invalid"));
            }
        }
        // Drop each `in_rows` guard before `db.live()`, which locks the same non-reentrant Mutex.
        let replayed = lk(&db.in_rows).get(&transfer_id).copied();
        if let Some(existing) = replayed {
            if existing != guid {
                return Err(anyhow!(
                    "transfer id already imported for another character"
                ));
            }
            if let Some(intent) = bot_arrival {
                let expected = (
                    intent.source_module_identity,
                    intent.id,
                    intent.controller_generation,
                    intent.created_micros,
                );
                if lk(&db.bot_arrivals).get(&transfer_id) != Some(&expected) {
                    return Err(anyhow!("destination arrival belongs to another crossing"));
                }
            }
            if lk(&db.arrival_sources).get(&transfer_id)
                != Some(&(source.map_id, source.instance_id, source.revision))
            {
                return Err(anyhow!(
                    "destination arrival belongs to another Realm crossing"
                ));
            }
            return Ok(());
        }
        if db.live(guid) {
            return Err(anyhow!("character {guid} is already live on this shard"));
        }
        // The destination rides IN the blob, exactly as the real `ExportBlob`'s `dest_*` fields do:
        // cross-database the blob is the only thing that reaches this side.
        lk(&db.characters).insert(guid, arriving);
        lk(&db.in_rows).insert(transfer_id, guid);
        lk(&db.arrival_sources).insert(
            transfer_id,
            (source.map_id, source.instance_id, source.revision),
        );
        if let Some(intent) = bot_arrival {
            lk(&db.bot_arrivals).insert(
                transfer_id,
                (
                    intent.source_module_identity,
                    intent.id,
                    intent.controller_generation,
                    intent.created_micros,
                ),
            );
        }
        Ok(())
    }

    /// `confirm_import`: the SOURCE-side attestation. Refuses without a local escrow.
    fn confirm_import(&self, transfer_id: u64) -> Result<()> {
        let db = self.xstep("confirm_import")?;
        let out = lk(&db.out_rows);
        let Some(escrow) = out.get(&transfer_id) else {
            return Err(anyhow!(
                "transfer {transfer_id}: nothing escrowed here to confirm"
            ));
        };
        lk(&db.in_rows).insert(transfer_id, escrow.character_guid);
        Ok(())
    }

    /// `plan_finish`: refuses while the in-row (the attestation) is absent — the guard that makes
    /// "zero durable copies" unreachable — then cascade-deletes the source copy, escrow last.
    fn finish_transfer(&self, transfer_id: u64) -> Result<()> {
        let db = self.xstep("finish_transfer")?;
        let mut out = lk(&db.out_rows);
        let Some(escrow) = out.get(&transfer_id).cloned() else {
            return Ok(()); // FinishPlan::AlreadyDone
        };
        if !lk(&db.in_rows).contains_key(&transfer_id) {
            return Err(anyhow!(
                "transfer {transfer_id}: not imported — refusing to release"
            ));
        }
        lk(&db.characters).remove(&escrow.character_guid);
        lk(&db.in_rows).remove(&transfer_id);
        out.remove(&transfer_id);
        Ok(())
    }

    /// `release_transfer`: refuses on a shard that is the SOURCE; replay-safe otherwise.
    fn release_transfer(&self, transfer_id: u64) -> Result<()> {
        let Some(_) = self.transfer.xdb.as_ref() else {
            return Ok(());
        };
        let db = self.xstep("release_transfer")?;
        if lk(&db.out_rows).contains_key(&transfer_id) {
            return Err(anyhow!(
                "transfer {transfer_id}: this database holds the SOURCE out-row"
            ));
        }
        if lk(&db.bot_arrivals).contains_key(&transfer_id) {
            return Err(anyhow!(
                "transfer {transfer_id}: session-less arrival is owned by its Transfer Intent"
            ));
        }
        lk(&db.in_rows).remove(&transfer_id);
        lk(&db.arrival_sources).remove(&transfer_id);
        Ok(())
    }

    fn release_player_transfer_arrival(
        &self,
        transfer_id: u64,
        character_guid: u64,
        source: crate::world::transfer::RealmLocatorPredecessor,
    ) -> Result<()> {
        let Some(db) = self.transfer.xdb.as_ref() else {
            return Ok(());
        };
        self.xstep("release_transfer")?;
        if transfer_id != character_guid || source.revision == 0 {
            return Err(anyhow!("player Transfer arrival identity is invalid"));
        }
        if lk(&db.in_rows).get(&transfer_id) != Some(&character_guid) {
            return Ok(());
        }
        if lk(&db.bot_arrivals).contains_key(&transfer_id)
            || lk(&db.arrival_sources).get(&transfer_id)
                != Some(&(source.map_id, source.instance_id, source.revision))
        {
            return Ok(());
        }
        lk(&db.in_rows).remove(&transfer_id);
        lk(&db.arrival_sources).remove(&transfer_id);
        Ok(())
    }

    fn transfer_arrival(
        &self,
        transfer_id: u64,
    ) -> Option<crate::world::transfer::TransferArrival> {
        let db = self.transfer.xdb.as_ref()?;
        let character_guid = *lk(&db.in_rows).get(&transfer_id)?;
        let (source, intent_id, generation, _) = lk(&db.bot_arrivals)
            .get(&transfer_id)
            .copied()
            .unwrap_or((spacetimedb_sdk::Identity::ZERO, 0, 0, 0));
        let (source_map, source_instance, source_locator_revision) = lk(&db.arrival_sources)
            .get(&transfer_id)
            .copied()
            .unwrap_or((0, 0, 0));
        Some(crate::world::transfer::TransferArrival {
            character_guid,
            source_map,
            source_instance,
            source_locator_revision,
            bot_source_identity: source,
            bot_transfer_intent_id: intent_id,
            bot_controller_generation: generation,
        })
    }

    fn begin_shard_index_transfer(
        &self,
        plan: &crate::world::transfer::TransferPlan,
        bot_intent: Option<(&crate::world::transfer::BotTransferIntent, u64)>,
    ) -> Result<crate::world::party::RealmCharacterPartition> {
        let mut phase = self.transfer.realm_partition.lock().unwrap();
        let Some(current) = *phase else {
            return Ok(crate::world::party::RealmCharacterPartition {
                map_id: 0,
                instance_id: 0,
                revision: bot_intent.map_or(1, |(intent, _)| intent.source_locator_revision.max(1)),
                transfer_pending: true,
                pending_destination_map: plan.dest_map_id,
                pending_destination_instance: plan.dest_instance_id,
                bot_source_identity: bot_intent
                    .map_or(spacetimedb_sdk::Identity::ZERO, |(intent, _)| {
                        intent.source_module_identity
                    }),
                bot_transfer_intent_id: bot_intent.map_or(0, |(intent, _)| intent.id),
                bot_controller_generation: bot_intent
                    .map_or(0, |(intent, _)| intent.controller_generation),
            });
        };
        let (source_revision, crossing) = bot_intent.map_or(
            (current.revision, (spacetimedb_sdk::Identity::ZERO, 0, 0)),
            |(intent, _)| {
                (
                    intent.source_locator_revision,
                    (
                        intent.source_module_identity,
                        intent.id,
                        intent.controller_generation,
                    ),
                )
            },
        );
        if !current.transfer_pending
            && (current.map_id, current.instance_id, current.revision)
                == (
                    plan.dest_map_id,
                    plan.dest_instance_id,
                    source_revision.saturating_add(1),
                )
            && (
                current.bot_source_identity,
                current.bot_transfer_intent_id,
                current.bot_controller_generation,
            ) == crossing
        {
            return Ok(current);
        }
        if current.transfer_pending
            && current.revision == source_revision
            && bot_intent.is_none_or(|(intent, _)| {
                (current.map_id, current.instance_id) == (intent.source_map, intent.source_instance)
            })
            && (
                current.pending_destination_map,
                current.pending_destination_instance,
            ) == (plan.dest_map_id, plan.dest_instance_id)
            && (
                current.bot_source_identity,
                current.bot_transfer_intent_id,
                current.bot_controller_generation,
            ) == crossing
        {
            return Ok(current);
        }
        if current.transfer_pending || current.revision != source_revision {
            return Err(anyhow!("Transfer Realm locator changed"));
        }
        let pending = crate::world::party::RealmCharacterPartition {
            transfer_pending: true,
            pending_destination_map: plan.dest_map_id,
            pending_destination_instance: plan.dest_instance_id,
            bot_source_identity: crossing.0,
            bot_transfer_intent_id: crossing.1,
            bot_controller_generation: crossing.2,
            ..current
        };
        *phase = Some(pending);
        Ok(pending)
    }

    fn finish_player_shard_index_transfer(
        &self,
        plan: &crate::world::transfer::TransferPlan,
        _source_map: u32,
        _source_instance: u64,
        source_revision: u64,
    ) -> Result<()> {
        let mut phase = self.transfer.realm_partition.lock().unwrap();
        let Some(current) = *phase else {
            drop(phase);
            return self.publish_shard_index(
                plan.character_guid,
                plan.dest_map_id,
                plan.dest_instance_id,
            );
        };
        if !current.transfer_pending || current.revision != source_revision {
            return Err(anyhow!("Transfer Realm locator phase changed"));
        }
        *phase = Some(crate::world::party::RealmCharacterPartition {
            map_id: plan.dest_map_id,
            instance_id: plan.dest_instance_id,
            revision: source_revision + 1,
            transfer_pending: false,
            pending_destination_map: 0,
            pending_destination_instance: 0,
            ..current
        });
        Ok(())
    }

    fn bind_bot_transfer_locator(
        &self,
        _intent: &crate::world::transfer::BotTransferIntent,
        source_revision: u64,
        _claim_token: u64,
    ) -> Result<()> {
        self.rec("bind_bot_transfer_locator");
        let Some(current) = *self.transfer.realm_partition.lock().unwrap() else {
            return Ok(());
        };
        if current.transfer_pending || current.revision != source_revision {
            return Err(anyhow!("Transfer Intent Realm locator changed"));
        }
        Ok(())
    }

    fn publish_bot_shard_index(
        &self,
        intent: &crate::world::transfer::BotTransferIntent,
    ) -> Result<()> {
        self.rec("publish_bot_shard_index");
        let mut phase = self.transfer.realm_partition.lock().unwrap();
        let Some(current) = *phase else {
            drop(phase);
            return self.publish_shard_index(
                intent.bot_guid,
                intent.destination_map,
                intent.destination_instance,
            );
        };
        let settled_revision = intent
            .source_locator_revision
            .checked_add(1)
            .ok_or_else(|| anyhow!("Transfer Realm locator revision exhausted"))?;
        let crossing = (
            intent.source_module_identity,
            intent.id,
            intent.controller_generation,
        );
        if !current.transfer_pending
            && (current.map_id, current.instance_id, current.revision)
                == (
                    intent.destination_map,
                    intent.destination_instance,
                    settled_revision,
                )
            && (
                current.bot_source_identity,
                current.bot_transfer_intent_id,
                current.bot_controller_generation,
            ) == crossing
        {
            return Ok(());
        }
        if !current.transfer_pending
            || current.revision != intent.source_locator_revision
            || (
                current.pending_destination_map,
                current.pending_destination_instance,
            ) != (intent.destination_map, intent.destination_instance)
            || (
                current.bot_source_identity,
                current.bot_transfer_intent_id,
                current.bot_controller_generation,
            ) != crossing
        {
            return Err(anyhow!("Transfer Realm locator phase changed"));
        }
        *phase = Some(crate::world::party::RealmCharacterPartition {
            map_id: intent.destination_map,
            instance_id: intent.destination_instance,
            revision: settled_revision,
            transfer_pending: false,
            pending_destination_map: 0,
            pending_destination_instance: 0,
            ..current
        });
        Ok(())
    }

    fn finish_pending_shard_index_transfer(
        &self,
        character_guid: u64,
        destination_map: u32,
        destination_instance: u64,
        arrival: &crate::world::transfer::TransferArrival,
    ) -> Result<()> {
        if arrival.character_guid != character_guid {
            return Err(anyhow!("arrival fence names another Character"));
        }
        let mut phase = self.transfer.realm_partition.lock().unwrap();
        let Some(current) = *phase else {
            return Ok(());
        };
        if !current.transfer_pending
            || (current.map_id, current.instance_id, current.revision)
                != (
                    arrival.source_map,
                    arrival.source_instance,
                    arrival.source_locator_revision,
                )
            || (
                current.pending_destination_map,
                current.pending_destination_instance,
            ) != (destination_map, destination_instance)
            || (
                current.bot_source_identity,
                current.bot_transfer_intent_id,
                current.bot_controller_generation,
            ) != (
                arrival.bot_source_identity,
                arrival.bot_transfer_intent_id,
                arrival.bot_controller_generation,
            )
        {
            return Err(anyhow!("pending Realm Transfer phase changed"));
        }
        *phase = Some(crate::world::party::RealmCharacterPartition {
            map_id: destination_map,
            instance_id: destination_instance,
            revision: arrival.source_locator_revision + 1,
            transfer_pending: false,
            pending_destination_map: 0,
            pending_destination_instance: 0,
            ..current
        });
        Ok(())
    }

    fn sync_transfer_arrival(&self, character_guid: u64) -> Result<()> {
        crate::world::party::sync_transfer_arrival_mirror(self, character_guid)?;
        self.xstep("sync_transfer_arrival")?;
        Ok(())
    }

    fn sync_transfer_pending(&self, character_guid: u64) -> Result<()> {
        crate::world::party::sync_transfer_arrival_mirror(self, character_guid)?;
        self.xstep("sync_transfer_pending")?;
        Ok(())
    }

    fn mark_bot_transfer_arrival_ready(
        &self,
        _intent_id: u64,
        _bot_guid: u64,
        _controller_generation: u64,
        _claim_token: u64,
    ) -> Result<()> {
        self.xstep("mark_bot_transfer_arrival_ready")?;
        Ok(())
    }

    fn bot_transfer_arrival_matches(
        &self,
        transfer_id: u64,
        intent: &crate::world::transfer::BotTransferIntent,
    ) -> bool {
        let Some(db) = self.transfer.xdb.as_ref() else {
            return false;
        };
        lk(&db.in_rows).get(&transfer_id) == Some(&intent.bot_guid)
            && lk(&db.bot_arrivals).get(&transfer_id)
                == Some(&(
                    intent.source_module_identity,
                    intent.id,
                    intent.controller_generation,
                    intent.created_micros,
                ))
            && lk(&db.arrival_sources).get(&transfer_id)
                == Some(&(
                    intent.source_map,
                    intent.source_instance,
                    intent.source_locator_revision,
                ))
    }

    fn release_bot_transfer_arrival(
        &self,
        transfer_id: u64,
        intent: &crate::world::transfer::BotTransferIntent,
    ) -> Result<()> {
        self.xstep("release_bot_transfer_arrival")?;
        let Some(db) = self.transfer.xdb.as_ref() else {
            return Ok(());
        };
        if transfer_id != intent.bot_guid
            || intent.source_module_identity == spacetimedb_sdk::Identity::ZERO
            || intent.id == 0
            || intent.created_micros <= 0
        {
            return Err(anyhow!("bot Transfer arrival identity is invalid"));
        }
        let expected = (
            intent.source_module_identity,
            intent.id,
            intent.controller_generation,
            intent.created_micros,
        );
        if lk(&db.bot_arrivals).get(&transfer_id) == Some(&expected) {
            if lk(&db.arrival_sources).get(&transfer_id)
                != Some(&(
                    intent.source_map,
                    intent.source_instance,
                    intent.source_locator_revision,
                ))
            {
                return Ok(());
            }
            lk(&db.in_rows).remove(&transfer_id);
            lk(&db.bot_arrivals).remove(&transfer_id);
            lk(&db.arrival_sources).remove(&transfer_id);
        }
        Ok(())
    }

    fn instance_partition(&self, instance_id: u64) -> Option<(u32, u64)> {
        self.transfer
            .xdb
            .as_ref()
            .and_then(|db| lk(&db.instance_partitions).get(&instance_id).copied())
    }

    fn ensure_instance(&self, instance_id: u64, map_id: u32, party_id: u64) -> Result<()> {
        let db = self.xstep("ensure_instance")?;
        if instance_id == 0 {
            return Err(anyhow!("instance 0 is the open world"));
        }
        let existing = lk(&db.instance_partitions).get(&instance_id).copied();
        match existing {
            Some((existing_map, _)) if existing_map != map_id => {
                return Err(anyhow!(
                    "instance {instance_id} belongs to map {existing_map}, not map {map_id}"
                ));
            }
            Some((_, existing_party))
                if existing_party != party_id && !(existing_party == 0 && party_id != 0) =>
            {
                return Err(anyhow!(
                    "instance {instance_id} belongs to party {existing_party}, not party {party_id}"
                ));
            }
            _ => {}
        }
        lk(&db.instance_partitions).insert(instance_id, (map_id, party_id));
        // The module's own shape: a mirror of an instance that is ALREADY here joins it (early
        // return) instead of spawning a second population. `HashSet::insert` reports that for free,
        // and the count is what the second-party-member test asserts against.
        if lk(&db.instances).insert(instance_id) {
            lk(&db.populated).push(instance_id);
        }
        Ok(())
    }

    fn evict_instance_population(&self, instance_id: u64) -> Result<()> {
        let db = self.xstep("evict_instance_population")?;
        if instance_id == 0 {
            return Err(anyhow!("instance 0 is the open world"));
        }
        lk(&db.evicted).push(instance_id);
        Ok(())
    }
}

// The cross-database transfer fixtures. `TransferStore` above runs on them through `xdb`, and
// `transfer_tests.rs` and the world-port tests build them directly.

/// One character's durable state, reduced to what a transfer has to preserve: where it is, and a
/// PAYLOAD marker standing for the character-owned rows (gear/spells/skills/quest log). If the
/// payload does not arrive, the character arrived NAKED — the failure a manifest-only blob has.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FakeChar {
    pub(crate) map_id: u32,
    pub(crate) instance_id: u64,
    pub(crate) payload: String,
}

#[derive(Clone, Debug)]
pub(crate) struct FakeEscrow {
    pub(crate) transfer_id: u64,
    pub(crate) character_guid: u64,
    pub(crate) dest_map_id: u32,
    pub(crate) dest_instance_id: u64,
    pub(crate) blob: Vec<u8>,
}

/// One SpacetimeDB database, as far as the transfer protocol can see it.
#[derive(Default)]
pub(crate) struct FakeShardDb {
    pub(crate) characters: std::sync::Mutex<std::collections::HashMap<u64, FakeChar>>,
    /// transfer_id → the source escrow (`game_transfer_out`).
    pub(crate) out_rows: std::sync::Mutex<std::collections::HashMap<u64, FakeEscrow>>,
    /// transfer_id → character guid (`game_transfer_in`): on the DESTINATION the arrival copy's
    /// fence, on the SOURCE the gateway's `confirm_import` attestation.
    pub(crate) in_rows: std::sync::Mutex<std::collections::HashMap<u64, u64>>,
    /// Exact bot Transfer identity attached to a destination fence.
    pub(crate) bot_arrivals: std::sync::Mutex<
        std::collections::HashMap<u64, (spacetimedb_sdk::Identity, u64, u64, i64)>,
    >,
    /// Realm locator predecessor attached to each destination fence.
    pub(crate) arrival_sources: std::sync::Mutex<std::collections::HashMap<u64, (u32, u64, u64)>>,
    pub(crate) instances: std::sync::Mutex<std::collections::HashSet<u64>>,
    pub(crate) instance_partitions: std::sync::Mutex<std::collections::HashMap<u64, (u32, u64)>>,
    /// Every instance id this database actually SPAWNED a population for — one entry per
    /// spawn, so "the second party member re-created the dungeon" is visible as a duplicate.
    pub(crate) populated: std::sync::Mutex<Vec<u64>>,
    pub(crate) evicted: std::sync::Mutex<Vec<u64>>,
}

impl FakeShardDb {
    pub(crate) fn with_character(guid: u64, c: FakeChar) -> std::sync::Arc<Self> {
        let db = Self::default();
        if c.instance_id != 0 {
            lk(&db.instances).insert(c.instance_id);
            lk(&db.instance_partitions).insert(c.instance_id, (c.map_id, 0));
        }
        lk(&db.characters).insert(guid, c);
        std::sync::Arc::new(db)
    }
    pub(crate) fn empty() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self::default())
    }
    pub(crate) fn has(&self, guid: u64) -> bool {
        lk(&self.characters).contains_key(&guid)
    }
    pub(crate) fn get(&self, guid: u64) -> Option<FakeChar> {
        lk(&self.characters).get(&guid).cloned()
    }
    /// A character is LIVE here iff it is durable AND neither escrow row fences it — the module's
    /// `login_allowed` predicate, which is what `player_login` actually gates on.
    pub(crate) fn live(&self, guid: u64) -> bool {
        self.has(guid)
            && !lk(&self.out_rows)
                .values()
                .any(|e| e.character_guid == guid)
            && !lk(&self.in_rows).values().any(|g| *g == guid)
    }
    pub(crate) fn settled(&self) -> bool {
        lk(&self.out_rows).is_empty() && lk(&self.in_rows).is_empty()
    }
}

/// The blob a fake `begin_transfer` produces. Carries the destination and the payload, exactly as
/// the real `ExportBlob` carries `dest_*` + `character_row` + `payload`: cross-database the blob is
/// the ONLY thing that reaches the far side.
pub(crate) fn fake_blob(guid: u64, dest_map: u32, dest_instance: u64, payload: &str) -> Vec<u8> {
    format!("{guid}|{dest_map}|{dest_instance}|{payload}").into_bytes()
}

pub(crate) fn parse_blob(blob: &[u8]) -> (u64, FakeChar) {
    let s = String::from_utf8(blob.to_vec()).expect("fake blob is utf8");
    let parts: Vec<&str> = s.splitn(4, '|').collect();
    (
        parts[0].parse().expect("guid"),
        FakeChar {
            map_id: parts[1].parse().expect("map"),
            instance_id: parts[2].parse().expect("instance"),
            payload: parts[3].to_string(),
        },
    )
}

impl WorldFake {
    /// Record a transfer step and honour an injected kill. `Err` means "the gateway died
    /// before this step's transaction committed", which is exactly a truncated drive.
    pub(crate) fn xstep(&self, what: &str) -> Result<&std::sync::Arc<FakeShardDb>> {
        let db = self
            .transfer
            .xdb
            .as_ref()
            .ok_or_else(|| anyhow!("this store does not implement cross-database transfers"))?;
        if self.transfer.kill_at.as_deref() == Some(what) {
            return Err(anyhow!("gateway killed at {what}"));
        }
        self.rec(what);
        Ok(db)
    }

    /// The realm-core index publish. Recorded in the shared call log so its POSITION in the
    /// drive is assertable, not just its effect.
    pub(crate) fn publish_shard_index(
        &self,
        character_guid: u64,
        map_id: u32,
        instance_id: u64,
    ) -> Result<()> {
        if let Some(e) = &self.transfer.publish_error {
            return Err(anyhow!("{e}"));
        }
        // Through `xstep`, like every other step of the drive — NOT a bare `rec`. Every other
        // transfer method routes its "gateway killed here" injection through it, and this one
        // originally did not, so `kill_at = "publish_shard_index"` was silently inert and the
        // crash matrix reported a PASS for a boundary it never killed at.
        self.xstep("publish_shard_index")?;
        self.transfer
            .realm_index
            .lock()
            .unwrap()
            .push((character_guid, map_id, instance_id));
        Ok(())
    }
}
