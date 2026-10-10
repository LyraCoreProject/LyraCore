//! `Coordinator`'s [`CombatStore`] adapter.

use anyhow::Result;

use crate::stdb::bindings::*;
use crate::stdb::connection::call_reducer;
use crate::stdb::Coordinator;
use crate::world::{Actor, CombatStore};

impl CombatStore for Coordinator {
    /// Targets are attributed to the caller through the coordinator connection. `target_guid` 0
    /// clears the selection.
    fn set_target(&self, actor: Actor, target_guid: u64) -> Result<()> {
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_set_target",
            gw_set_target_then(self.session_actor(actor), target_guid)
        )
    }

    fn pet_command(&self, actor: Actor, data: u32, target_guid: u64) -> Result<()> {
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_pet_command",
            gw_pet_command_then(self.session_actor(actor), data, target_guid)
        )
    }

    fn set_sheathed(&self, actor: Actor, state: u8) -> Result<()> {
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_set_sheathed",
            gw_set_sheathed_then(self.session_actor(actor), state)
        )
    }
}
