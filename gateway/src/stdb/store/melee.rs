//! `Coordinator`'s [`MeleeActionStore`] adapter.

use anyhow::{anyhow, Result};

use crate::stdb::bindings::*;
use crate::stdb::connection::call_reducer;
use crate::stdb::Coordinator;
use crate::world::{Actor, MeleeActionStore};

impl MeleeActionStore for crate::stdb::Coordinator {
    fn start_attack(&self, account_id: u64, actor_guid: u64, target_guid: u64) -> Result<()> {
        crate::stdb::Coordinator::start_attack(self, account_id, actor_guid, target_guid)
    }

    fn stop_attack(&self, account_id: u64, actor_guid: u64) -> Result<()> {
        crate::stdb::Coordinator::stop_attack(self, account_id, actor_guid)
    }
}

impl Coordinator {
    /// Start the player's melee auto-attack on `target_guid` (`CMSG_ATTACKSWING`, combat C1) over
    /// the coordinator connection so the module attributes the swing to the caller.
    pub fn start_attack(&self, _account_id: u64, actor_guid: u64, target_guid: u64) -> Result<()> {
        let actor =
            Actor::new(actor_guid).ok_or_else(|| anyhow!("start_attack: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_attack",
            gw_attack_then(self.session_actor(actor), target_guid)
        )
    }

    /// Stop the player's melee auto-attack (`CMSG_ATTACKSTOP`, combat C1).
    pub fn stop_attack(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        let actor =
            Actor::new(actor_guid).ok_or_else(|| anyhow!("stop_attack: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_stop_attack",
            gw_stop_attack_then(self.session_actor(actor))
        )
    }
}
