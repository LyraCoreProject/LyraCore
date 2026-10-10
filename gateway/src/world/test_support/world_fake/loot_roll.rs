use super::super::*;
use crate::stdb::ReducerCallError;

#[derive(Default)]
pub(crate) struct LootRollState {
    /// Recorded `loot_roll` calls: (corpse_guid, loot_slot, vote).
    pub(crate) loot_rolls: std::sync::Mutex<Vec<(u64, u32, u8)>>,
    /// Recorded `loot_master_give` calls: (corpse_guid, loot_slot, target_guid).
    pub(crate) loot_master_gives: std::sync::Mutex<Vec<(u64, u8, u64)>>,
    /// Typed gameplay Refusal returned by loot-roll and master-loot action tests.
    pub(crate) loot_action_refusal: Option<LootRefusal>,
    /// When set, loot-roll and master-loot requests fail as a Transport Loss.
    pub(crate) loot_action_transport_lost: bool,
    /// Records every `realm_loot_op` reducer argument in wire order. The Realm-core handle owns the
    /// recorder so a test can distinguish authority routing from a Shard-local request.
    #[allow(clippy::type_complexity)]
    pub(crate) realm_loot_ops: std::sync::Mutex<
        Vec<(
            u8,
            u64,
            u8,
            u32,
            u64,
            u8,
            i64,
            Vec<u64>,
            u32,
            spacetimedb_sdk::Identity,
            u64,
        )>,
    >,
    /// When set, `realm_loot_start` is refused with this reason.
    pub(crate) realm_loot_op_refusal: Option<String>,
    /// This WORLD SHARD's staging rolls `pending_local_rolls` answers — the relay's promotion
    /// INPUT. `Mutex`-wrapped (like `mirror`) so a test can set it AFTER the fixture
    /// is wrapped in an `Arc` — every existing party/whisper topology builder hands back `Arc`s.
    /// Empty (derive-Default) = nothing to promote, byte-identical to before this field existed.
    pub(crate) pending_rolls: std::sync::Mutex<Vec<crate::world::loot::PendingLootRoll>>,
    /// Recorded `settle_loot_roll` calls on THIS shard — `(corpse_guid, slot, winner_guid)`.
    pub(crate) settled_rolls: std::sync::Mutex<Vec<(u64, u8, u64)>>,
    /// When set, `settle_loot_roll` fails as a Transport Loss.
    pub(crate) settle_loot_roll_transport_lost: bool,
    /// Recorded `clear_promoted_loot_roll` calls on THIS shard — the roll ids the relay told
    /// this shard's staging copy to forget after a successful promotion.
    pub(crate) cleared_rolls: std::sync::Mutex<Vec<u64>>,
    /// This REALM-CORE handle's fixture `ROLL_WON` queue — `(corpse_guid, slot, winner_guid)`
    /// triples, in the order they "arrived". `loot_won_since(after_id)` answers every entry whose
    /// 1-based INDEX exceeds `after_id`, and the new watermark is the queue's length — the same
    /// shape the real `game_group_event.id` high-water mark has, without needing a fake event table.
    /// `Mutex`-wrapped for the same after-`Arc`-construction reason as `pending_rolls`.
    pub(crate) won_events: std::sync::Mutex<Vec<(u64, u8, u64)>>,
}

impl LootRollStore for WorldFake {
    fn loot_roll(
        &self,
        _actor: Actor,
        corpse_guid: u64,
        loot_slot: u32,
        vote: u8,
    ) -> Result<LootActionStatus> {
        if self.loot_roll.loot_action_transport_lost {
            return Err(ReducerCallError::transport_lost("gw_loot_roll").into());
        }
        if let Some(refusal) = self.loot_roll.loot_action_refusal {
            return Ok(LootActionStatus::Refused(refusal));
        }
        self.loot_roll
            .loot_rolls
            .lock()
            .unwrap()
            .push((corpse_guid, loot_slot, vote));
        Ok(LootActionStatus::Applied)
    }

    fn loot_master_give(
        &self,
        _actor: Actor,
        corpse_guid: u64,
        loot_slot: u8,
        target_guid: u64,
    ) -> Result<LootActionStatus> {
        if self.loot_roll.loot_action_transport_lost {
            return Err(ReducerCallError::transport_lost("gw_loot_master_give").into());
        }
        if let Some(refusal) = self.loot_roll.loot_action_refusal {
            return Ok(LootActionStatus::Refused(refusal));
        }
        self.loot_roll.loot_master_gives.lock().unwrap().push((
            corpse_guid,
            loot_slot,
            target_guid,
        ));
        Ok(LootActionStatus::Applied)
    }

    #[allow(clippy::too_many_arguments)]
    fn realm_loot_start(
        &self,
        corpse_guid: u64,
        slot: u8,
        item_entry: u32,
        deadline_micros: i64,
        recipients: Vec<u64>,
        random_property_id: u32,
        promotion_source: spacetimedb_sdk::Identity,
        source_roll_id: u64,
    ) -> Result<()> {
        self.rec("realm_loot_op");
        self.loot_roll.realm_loot_ops.lock().unwrap().push((
            lyracore_shared::loot_roll::loot_op::START,
            corpse_guid,
            slot,
            item_entry,
            0,
            0,
            deadline_micros,
            recipients,
            random_property_id,
            promotion_source,
            source_roll_id,
        ));
        match &self.loot_roll.realm_loot_op_refusal {
            Some(reason) => Err(ReducerCallError::refused("realm_loot_op", reason).into()),
            None => Ok(()),
        }
    }

    fn realm_loot_vote(
        &self,
        corpse_guid: u64,
        slot: u8,
        actor: Actor,
        vote: u8,
    ) -> Result<LootActionStatus> {
        self.rec("realm_loot_op");
        self.loot_roll.realm_loot_ops.lock().unwrap().push((
            lyracore_shared::loot_roll::loot_op::VOTE,
            corpse_guid,
            slot,
            0,
            actor.guid(),
            vote,
            0,
            Vec::new(),
            0,
            spacetimedb_sdk::Identity::ZERO,
            0,
        ));
        if self.loot_roll.loot_action_transport_lost {
            return Err(ReducerCallError::transport_lost("realm_loot_op").into());
        }
        Ok(self
            .loot_roll
            .loot_action_refusal
            .map_or(LootActionStatus::Applied, LootActionStatus::Refused))
    }

    fn pending_local_rolls(&self) -> Result<Vec<crate::world::loot::PendingLootRoll>> {
        Ok(self.loot_roll.pending_rolls.lock().unwrap().clone())
    }

    fn settle_loot_roll(&self, corpse_guid: u64, slot: u8, winner_guid: u64) -> Result<()> {
        self.rec("settle_loot_roll");
        if self.loot_roll.settle_loot_roll_transport_lost {
            return Err(ReducerCallError::transport_lost("settle_loot_roll").into());
        }
        self.loot_roll
            .settled_rolls
            .lock()
            .unwrap()
            .push((corpse_guid, slot, winner_guid));
        Ok(())
    }

    fn clear_promoted_loot_roll(&self, roll_id: u64) -> Result<()> {
        self.rec("clear_promoted_loot_roll");
        self.loot_roll.cleared_rolls.lock().unwrap().push(roll_id);
        self.loot_roll
            .pending_rolls
            .lock()
            .unwrap()
            .retain(|r| r.roll_id != roll_id);
        Ok(())
    }

    fn loot_won_since(&self, after_id: u64) -> Result<(u64, Vec<(u64, u8, u64)>)> {
        let events = self.loot_roll.won_events.lock().unwrap();
        let watermark = events.len() as u64;
        let wins = events
            .iter()
            .enumerate()
            .filter(|(i, _)| (*i as u64 + 1) > after_id)
            .map(|(_, w)| *w)
            .collect();
        Ok((watermark, wins))
    }
}
