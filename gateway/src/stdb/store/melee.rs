//! `Coordinator`'s [`MeleeActionStore`] adapter.

use anyhow::Result;

use crate::stdb::bindings::*;
use crate::stdb::connection::call_reducer;
use crate::stdb::Coordinator;
use crate::world::{Actor, MeleeActionStore};

impl MeleeActionStore for Coordinator {
    /// Start the player's melee auto-attack on `target_guid` (`CMSG_ATTACKSWING`, combat C1) over
    /// the coordinator connection so the module attributes the swing to the caller.
    fn start_attack(&self, actor: Actor, target_guid: u64) -> Result<()> {
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_attack",
            gw_attack_then(self.session_actor(actor), target_guid)
        )
    }

    /// Stop the player's melee auto-attack (`CMSG_ATTACKSTOP`, combat C1).
    fn stop_attack(&self, actor: Actor) -> Result<()> {
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_stop_attack",
            gw_stop_attack_then(self.session_actor(actor))
        )
    }
}
