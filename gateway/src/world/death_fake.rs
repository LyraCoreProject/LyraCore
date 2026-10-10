//! A Fake of the death family's Store that models only who is alive.

use crate::stdb::ReducerCallError;
use crate::world::handlers::DeathStore;
use crate::world::Actor;
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Life {
    Dead,
    Ghost,
    Alive,
}

#[derive(Default)]
pub(crate) struct DeathFake {
    life: Mutex<HashMap<u64, Life>>,
    /// Owner guid to the guid of the corpse that owner can reclaim.
    corpses: HashMap<u64, u64>,
    /// Characters with an unanswered resurrect offer.
    res_offers: Mutex<HashSet<u64>>,
    /// Characters holding an unspent Self-Resurrection Option.
    self_res_options: Mutex<HashSet<u64>>,
    spirit_healers: HashSet<u64>,
    /// When set, every Durable Request fails as a Transport Loss.
    transport_lost: bool,
}

impl DeathFake {
    pub(crate) fn with_life(self, guid: u64, life: Life) -> Self {
        self.life.lock().unwrap().insert(guid, life);
        self
    }

    pub(crate) fn with_corpse(mut self, owner: u64, corpse: u64) -> Self {
        self.corpses.insert(owner, corpse);
        self
    }

    pub(crate) fn with_res_offer(self, guid: u64) -> Self {
        self.res_offers.lock().unwrap().insert(guid);
        self
    }

    pub(crate) fn with_self_res_option(self, guid: u64) -> Self {
        self.self_res_options.lock().unwrap().insert(guid);
        self
    }

    pub(crate) fn with_spirit_healer(mut self, guid: u64) -> Self {
        self.spirit_healers.insert(guid);
        self
    }

    pub(crate) fn with_transport_loss(mut self) -> Self {
        self.transport_lost = true;
        self
    }

    /// Characters nobody set up are alive.
    pub(crate) fn life(&self, guid: u64) -> Life {
        self.life
            .lock()
            .unwrap()
            .get(&guid)
            .copied()
            .unwrap_or(Life::Alive)
    }

    pub(crate) fn has_res_offer(&self, guid: u64) -> bool {
        self.res_offers.lock().unwrap().contains(&guid)
    }

    pub(crate) fn has_self_res_option(&self, guid: u64) -> bool {
        self.self_res_options.lock().unwrap().contains(&guid)
    }

    /// The Module refuses a request the character's state does not allow.
    fn require(&self, operation: &str, guid: u64, needed: Life) -> Result<()> {
        self.reachable(operation)?;
        match self.life(guid) {
            life if life == needed => Ok(()),
            life => Err(ReducerCallError::refused(
                operation,
                &format!("character {guid} is {life:?}, not {needed:?}"),
            )
            .into()),
        }
    }

    fn reachable(&self, operation: &str) -> Result<()> {
        if self.transport_lost {
            return Err(ReducerCallError::transport_lost(operation).into());
        }
        Ok(())
    }

    fn revive(&self, guid: u64) {
        self.life.lock().unwrap().insert(guid, Life::Alive);
    }
}

impl DeathStore for DeathFake {
    fn repop(&self, actor: Actor) -> Result<()> {
        let self_guid = actor.guid();
        self.require("gw_repop", self_guid, Life::Dead)?;
        self.revive(self_guid);
        Ok(())
    }

    fn reclaim_corpse(&self, actor: Actor, corpse_guid: u64) -> Result<()> {
        let self_guid = actor.guid();
        self.require("gw_reclaim_corpse", self_guid, Life::Ghost)?;
        if self.corpses.get(&self_guid) != Some(&corpse_guid) {
            return Err(ReducerCallError::refused(
                "gw_reclaim_corpse",
                &format!("corpse {corpse_guid} is not character {self_guid}'s"),
            )
            .into());
        }
        self.revive(self_guid);
        Ok(())
    }

    fn resurrect_response(&self, actor: Actor, accept: bool) -> Result<()> {
        let self_guid = actor.guid();
        self.reachable("gw_respond_resurrect")?;
        if !self.res_offers.lock().unwrap().remove(&self_guid) {
            return Err(ReducerCallError::refused(
                "gw_respond_resurrect",
                &format!("character {self_guid} has no pending offer"),
            )
            .into());
        }
        if accept {
            self.revive(self_guid);
        }
        Ok(())
    }

    fn self_resurrect(&self, actor: Actor) -> Result<()> {
        let self_guid = actor.guid();
        self.require("gw_self_resurrect", self_guid, Life::Dead)?;
        if !self.self_res_options.lock().unwrap().remove(&self_guid) {
            return Err(ReducerCallError::refused(
                "gw_self_resurrect",
                "no Self-Resurrection Option",
            )
            .into());
        }
        self.revive(self_guid);
        Ok(())
    }

    fn spirit_healer_res(&self, actor: Actor, healer_guid: u64) -> Result<()> {
        let self_guid = actor.guid();
        self.require("gw_spirit_res", self_guid, Life::Ghost)?;
        if !self.spirit_healers.contains(&healer_guid) {
            return Err(ReducerCallError::refused(
                "gw_spirit_res",
                &format!("{healer_guid} is not a Spirit Healer"),
            )
            .into());
        }
        self.revive(self_guid);
        Ok(())
    }

    fn corpse_location(&self, _owner_guid: u64) -> Result<Option<(u32, f32, f32, f32)>> {
        unreachable!("no test asks for a corpse location")
    }
}
