//! `Coordinator`'s [`DeathStore`] adapter.

use anyhow::Result;
use spacetimedb_sdk::Table;

use crate::stdb::bindings::*;
use crate::stdb::connection::call_reducer;
use crate::stdb::Coordinator;
use crate::world::{Actor, DeathStore};

impl DeathStore for Coordinator {
    fn repop(&self, actor: Actor) -> Result<()> {
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_repop",
            gw_repop_then(self.session_actor(actor))
        )
    }

    fn reclaim_corpse(&self, actor: Actor, corpse_guid: u64) -> Result<()> {
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_reclaim_corpse",
            gw_reclaim_corpse_then(self.session_actor(actor), corpse_guid)
        )
    }

    fn resurrect_response(&self, actor: Actor, accept: bool) -> Result<()> {
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_respond_resurrect",
            gw_respond_resurrect_then(self.session_actor(actor), accept)
        )
    }

    fn self_resurrect(&self, actor: Actor) -> Result<()> {
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_self_resurrect",
            gw_self_resurrect_then(self.session_actor(actor))
        )
    }

    /// `gw_spirit_res` takes no healer guid: the Module revives the ghost in place.
    fn spirit_healer_res(&self, actor: Actor, _healer_guid: u64) -> Result<()> {
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_spirit_res",
            gw_spirit_res_then(self.session_actor(actor))
        )
    }

    /// Reads the corpse from the privileged cache. `None` when the owner has no corpse.
    fn corpse_location(&self, owner_guid: u64) -> Result<Option<(u32, f32, f32, f32)>> {
        Ok(self
            .0
            .coord()
            .conn
            .db
            .game_corpse()
            .iter()
            .find(|c| c.owner_guid == owner_guid)
            .map(|c| (c.map_id, c.x, c.y, c.z)))
    }
}
