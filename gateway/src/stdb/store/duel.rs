//! `Coordinator`'s [`DuelActionStore`] adapter.

use anyhow::{anyhow, Result};

use crate::stdb::bindings::*;
use crate::stdb::connection::call_reducer;
use crate::stdb::Coordinator;
use crate::world::{Actor, DuelActionStore};

impl DuelActionStore for crate::stdb::Coordinator {
    fn duel_accept(&self, account_id: u64, actor_guid: u64, flag_guid: u64) -> Result<()> {
        crate::stdb::Coordinator::duel_accept(self, account_id, actor_guid, flag_guid)
    }

    fn duel_cancel(&self, account_id: u64, actor_guid: u64, flag_guid: u64) -> Result<()> {
        crate::stdb::Coordinator::duel_cancel(self, account_id, actor_guid, flag_guid)
    }
}

impl Coordinator {
    pub fn duel_accept(&self, _account_id: u64, actor_guid: u64, flag_guid: u64) -> Result<()> {
        let actor =
            Actor::new(actor_guid).ok_or_else(|| anyhow!("duel_accept: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_duel_accept",
            gw_duel_accept_then(self.session_actor(actor), flag_guid)
        )
    }

    pub fn duel_cancel(&self, _account_id: u64, actor_guid: u64, flag_guid: u64) -> Result<()> {
        let actor =
            Actor::new(actor_guid).ok_or_else(|| anyhow!("duel_cancel: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_duel_cancel",
            gw_duel_cancel_then(self.session_actor(actor), flag_guid)
        )
    }
}
