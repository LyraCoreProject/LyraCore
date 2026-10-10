//! `Coordinator`'s [`CombatStore`] adapter.

use anyhow::{anyhow, Result};

use crate::stdb::bindings::*;
use crate::stdb::connection::call_reducer;
use crate::stdb::Coordinator;
use crate::world::{Actor, CombatStore};

impl CombatStore for Coordinator {
    fn set_target(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()> {
        self.set_target(account_id, self_guid, target_guid)
    }

    fn pet_command(
        &self,
        account_id: u64,
        self_guid: u64,
        data: u32,
        target_guid: u64,
    ) -> Result<()> {
        self.pet_command(account_id, self_guid, data, target_guid)
    }

    fn set_sheathed(&self, account_id: u64, self_guid: u64, state: u8) -> Result<()> {
        self.set_sheathed(account_id, self_guid, state)
    }
}

impl Coordinator {
    /// Set the player's current target (`CMSG_SET_SELECTION`, Tier 2 / N3) over the coordinator
    /// connection so the module attributes it to the caller. `target_guid` 0 clears it.
    pub fn set_target(&self, _account_id: u64, actor_guid: u64, target_guid: u64) -> Result<()> {
        let actor =
            Actor::new(actor_guid).ok_or_else(|| anyhow!("set_target: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_set_target",
            gw_set_target_then(self.session_actor(actor), target_guid)
        )
    }

    /// Relay a pet command-bar action (`CMSG_PET_ACTION`) over the coordinator connection so the module
    /// attributes it to the pet's owner. `data` is the raw packed action (flag<<24 | id); the module
    /// decodes stay/follow/attack/dismiss + passive/defensive/aggressive.
    pub fn pet_command(
        &self,
        _account_id: u64,
        actor_guid: u64,
        data: u32,
        target_guid: u64,
    ) -> Result<()> {
        let actor =
            Actor::new(actor_guid).ok_or_else(|| anyhow!("pet_command: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_pet_command",
            gw_pet_command_then(self.session_actor(actor), data, target_guid)
        )
    }

    /// Draw or stow the player's weapons (`CMSG_SETSHEATHED`).
    pub fn set_sheathed(&self, _account_id: u64, actor_guid: u64, state: u8) -> Result<()> {
        let actor =
            Actor::new(actor_guid).ok_or_else(|| anyhow!("set_sheathed: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_set_sheathed",
            gw_set_sheathed_then(self.session_actor(actor), state)
        )
    }
}
