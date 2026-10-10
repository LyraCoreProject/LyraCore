//! `Coordinator`'s [`DeathStore`] adapter.

use anyhow::{anyhow, Result};
use spacetimedb_sdk::Table;

use crate::stdb::bindings::*;
use crate::stdb::connection::call_reducer;
use crate::stdb::Coordinator;
use crate::world::{Actor, DeathStore};

impl DeathStore for Coordinator {
    fn repop(&self, account_id: u64, self_guid: u64) -> Result<()> {
        self.repop(account_id, self_guid)
    }

    fn reclaim_corpse(&self, account_id: u64, self_guid: u64, corpse_guid: u64) -> Result<()> {
        self.reclaim_corpse(account_id, self_guid, corpse_guid)
    }

    fn resurrect_response(&self, account_id: u64, self_guid: u64, accept: bool) -> Result<()> {
        self.resurrect_response(account_id, self_guid, accept)
    }

    fn self_resurrect(&self, account_id: u64, self_guid: u64) -> Result<()> {
        self.self_resurrect(account_id, self_guid)
    }

    fn spirit_healer_res(&self, account_id: u64, self_guid: u64, healer_guid: u64) -> Result<()> {
        self.spirit_healer_res(account_id, self_guid, healer_guid)
    }

    fn corpse_location(&self, owner_guid: u64) -> Result<Option<(u32, f32, f32, f32)>> {
        self.corpse_location(owner_guid)
    }
}

impl Coordinator {
    /// Find `owner_guid`'s corpse location `(map_id, x, y, z)` from the privileged cache, for the
    /// `MSG_CORPSE_QUERY` reply. `None` if they have no corpse.
    pub fn corpse_location(&self, owner_guid: u64) -> Result<Option<(u32, f32, f32, f32)>> {
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

    /// Revive the caller after death (`CMSG_REPOP_REQUEST`) over the coordinator connection.
    /// Rides the coordinator connection as `gw_repop`.
    pub fn repop(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        let actor =
            Actor::new(actor_guid).ok_or_else(|| anyhow!("repop: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_repop",
            gw_repop_then(self.session_actor(actor))
        )
    }

    /// Reclaim the caller's corpse (`CMSG_RECLAIM_CORPSE`) over the coordinator connection.
    pub fn reclaim_corpse(
        &self,
        _account_id: u64,
        actor_guid: u64,
        corpse_guid: u64,
    ) -> Result<()> {
        let actor = Actor::new(actor_guid)
            .ok_or_else(|| anyhow!("reclaim_corpse: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_reclaim_corpse",
            gw_reclaim_corpse_then(self.session_actor(actor), corpse_guid)
        )
    }

    /// Answer a pending resurrect offer (`CMSG_RESURRECT_RESPONSE`) over the coordinator connection.
    /// Rides the coordinator connection as `gw_respond_resurrect`.
    pub fn resurrect_response(
        &self,
        _account_id: u64,
        actor_guid: u64,
        accept: bool,
    ) -> Result<()> {
        let actor = Actor::new(actor_guid)
            .ok_or_else(|| anyhow!("resurrect_response: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_respond_resurrect",
            gw_respond_resurrect_then(self.session_actor(actor), accept)
        )
    }

    /// Use the caller's Self-Resurrection Option (`CMSG_SELF_RES`) over the coordinator connection.
    /// Rides the coordinator connection as `gw_self_resurrect`.
    pub fn self_resurrect(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        let actor = Actor::new(actor_guid)
            .ok_or_else(|| anyhow!("self_resurrect: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_self_resurrect",
            gw_self_resurrect_then(self.session_actor(actor))
        )
    }

    /// Spirit-Healer resurrect (`CMSG_SPIRIT_HEALER_ACTIVATE`) over the coordinator connection: the
    /// module res's the caller in place at 50% + applies Resurrection Sickness if it's a ghost.
    /// `gw_spirit_res` takes no `healer_guid`.
    pub fn spirit_healer_res(
        &self,
        _account_id: u64,
        actor_guid: u64,
        _healer_guid: u64,
    ) -> Result<()> {
        let actor = Actor::new(actor_guid)
            .ok_or_else(|| anyhow!("spirit_healer_res: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_spirit_res",
            gw_spirit_res_then(self.session_actor(actor))
        )
    }
}
