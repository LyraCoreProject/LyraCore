//! `Coordinator`'s [`TradeStore`] adapter.

use anyhow::Result;

use crate::stdb::bindings::*;
use crate::stdb::connection::call_reducer;
use crate::world::{Actor, TradeStore};

impl TradeStore for crate::stdb::Coordinator {
    fn initiate_trade(&self, actor: Actor, target_guid: u64) -> Result<()> {
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_initiate_trade",
            gw_initiate_trade_then(self.session_actor(actor), target_guid)
        )
    }

    fn begin_trade(&self, actor: Actor) -> Result<()> {
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_begin_trade",
            gw_begin_trade_then(self.session_actor(actor))
        )
    }

    fn cancel_trade(&self, actor: Actor) -> Result<()> {
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_cancel_trade",
            gw_cancel_trade_then(self.session_actor(actor))
        )
    }

    fn set_trade_item(&self, actor: Actor, trade_slot: u8, inv_slot: u8) -> Result<()> {
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_set_trade_item",
            gw_set_trade_item_then(self.session_actor(actor), trade_slot, inv_slot)
        )
    }

    fn clear_trade_item(&self, actor: Actor, trade_slot: u8) -> Result<()> {
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_clear_trade_item",
            gw_clear_trade_item_then(self.session_actor(actor), trade_slot)
        )
    }

    fn set_trade_gold(&self, actor: Actor, copper: u32) -> Result<()> {
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_set_trade_gold",
            gw_set_trade_gold_then(self.session_actor(actor), copper)
        )
    }

    fn accept_trade(&self, actor: Actor) -> Result<()> {
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_accept_trade",
            gw_accept_trade_then(self.session_actor(actor))
        )
    }

    fn unaccept_trade(&self, actor: Actor) -> Result<()> {
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_unaccept_trade",
            gw_unaccept_trade_then(self.session_actor(actor))
        )
    }

    fn busy_trade(&self, actor: Actor) -> Result<()> {
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_busy_trade",
            gw_busy_trade_then(self.session_actor(actor))
        )
    }

    fn ignore_trade(&self, actor: Actor) -> Result<()> {
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_ignore_trade",
            gw_ignore_trade_then(self.session_actor(actor))
        )
    }
}
