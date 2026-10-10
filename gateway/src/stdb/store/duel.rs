//! `Coordinator`'s [`DuelActionStore`] adapter.

use anyhow::Result;

use crate::stdb::bindings::*;
use crate::stdb::connection::call_reducer;
use crate::world::{Actor, DuelActionStore};

impl DuelActionStore for crate::stdb::Coordinator {
    fn duel_accept(&self, actor: Actor, flag_guid: u64) -> Result<()> {
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_duel_accept",
            gw_duel_accept_then(self.session_actor(actor), flag_guid)
        )
    }

    fn duel_cancel(&self, actor: Actor, flag_guid: u64) -> Result<()> {
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_duel_cancel",
            gw_duel_cancel_then(self.session_actor(actor), flag_guid)
        )
    }
}
