//! `Coordinator`'s [`TradeStore`] adapter.

use anyhow::{anyhow, Result};

use crate::stdb::bindings::*;
use crate::stdb::connection::call_reducer;
use crate::stdb::Coordinator;
use crate::world::{Actor, TradeStore};

impl TradeStore for Coordinator {
    fn initiate_trade(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()> {
        self.initiate_trade(account_id, self_guid, target_guid)
    }

    fn begin_trade(&self, account_id: u64, self_guid: u64) -> Result<()> {
        self.begin_trade(account_id, self_guid)
    }

    fn cancel_trade(&self, account_id: u64, self_guid: u64) -> Result<()> {
        self.cancel_trade(account_id, self_guid)
    }

    fn set_trade_item(
        &self,
        account_id: u64,
        self_guid: u64,
        trade_slot: u8,
        inv_slot: u8,
    ) -> Result<()> {
        self.set_trade_item(account_id, self_guid, trade_slot, inv_slot)
    }

    fn clear_trade_item(&self, account_id: u64, self_guid: u64, trade_slot: u8) -> Result<()> {
        self.clear_trade_item(account_id, self_guid, trade_slot)
    }

    fn set_trade_gold(&self, account_id: u64, self_guid: u64, copper: u32) -> Result<()> {
        self.set_trade_gold(account_id, self_guid, copper)
    }

    fn accept_trade(&self, account_id: u64, self_guid: u64) -> Result<()> {
        self.accept_trade(account_id, self_guid)
    }

    fn unaccept_trade(&self, account_id: u64, self_guid: u64) -> Result<()> {
        self.unaccept_trade(account_id, self_guid)
    }

    fn busy_trade(&self, account_id: u64, self_guid: u64) -> Result<()> {
        self.busy_trade(account_id, self_guid)
    }

    fn ignore_trade(&self, account_id: u64, self_guid: u64) -> Result<()> {
        self.ignore_trade(account_id, self_guid)
    }
}

impl Coordinator {
    /// `CMSG_INITIATE_TRADE`, `target_guid` is the client's targeted player.
    pub fn initiate_trade(
        &self,
        _account_id: u64,
        actor_guid: u64,
        target_guid: u64,
    ) -> Result<()> {
        let actor = Actor::new(actor_guid)
            .ok_or_else(|| anyhow!("initiate_trade: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_initiate_trade",
            gw_initiate_trade_then(self.session_actor(actor), target_guid)
        )
    }

    /// `CMSG_BEGIN_TRADE`.
    pub fn begin_trade(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        let actor =
            Actor::new(actor_guid).ok_or_else(|| anyhow!("begin_trade: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_begin_trade",
            gw_begin_trade_then(self.session_actor(actor))
        )
    }

    /// `CMSG_CANCEL_TRADE`.
    pub fn cancel_trade(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        let actor =
            Actor::new(actor_guid).ok_or_else(|| anyhow!("cancel_trade: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_cancel_trade",
            gw_cancel_trade_then(self.session_actor(actor))
        )
    }

    /// `CMSG_SET_TRADE_ITEM`.
    pub fn set_trade_item(
        &self,
        _account_id: u64,
        actor_guid: u64,
        trade_slot: u8,
        inv_slot: u8,
    ) -> Result<()> {
        let actor = Actor::new(actor_guid)
            .ok_or_else(|| anyhow!("set_trade_item: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_set_trade_item",
            gw_set_trade_item_then(self.session_actor(actor), trade_slot, inv_slot)
        )
    }

    /// `CMSG_CLEAR_TRADE_ITEM`.
    pub fn clear_trade_item(
        &self,
        _account_id: u64,
        actor_guid: u64,
        trade_slot: u8,
    ) -> Result<()> {
        let actor = Actor::new(actor_guid)
            .ok_or_else(|| anyhow!("clear_trade_item: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_clear_trade_item",
            gw_clear_trade_item_then(self.session_actor(actor), trade_slot)
        )
    }

    /// `CMSG_SET_TRADE_GOLD`.
    pub fn set_trade_gold(&self, _account_id: u64, actor_guid: u64, copper: u32) -> Result<()> {
        let actor = Actor::new(actor_guid)
            .ok_or_else(|| anyhow!("set_trade_gold: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_set_trade_gold",
            gw_set_trade_gold_then(self.session_actor(actor), copper)
        )
    }

    /// `CMSG_ACCEPT_TRADE`.
    pub fn accept_trade(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        let actor =
            Actor::new(actor_guid).ok_or_else(|| anyhow!("accept_trade: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_accept_trade",
            gw_accept_trade_then(self.session_actor(actor))
        )
    }

    /// `CMSG_UNACCEPT_TRADE`.
    pub fn unaccept_trade(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        let actor = Actor::new(actor_guid)
            .ok_or_else(|| anyhow!("unaccept_trade: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_unaccept_trade",
            gw_unaccept_trade_then(self.session_actor(actor))
        )
    }

    /// `CMSG_BUSY_TRADE`.
    pub fn busy_trade(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        let actor =
            Actor::new(actor_guid).ok_or_else(|| anyhow!("busy_trade: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_busy_trade",
            gw_busy_trade_then(self.session_actor(actor))
        )
    }

    /// `CMSG_IGNORE_TRADE`.
    pub fn ignore_trade(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        let actor =
            Actor::new(actor_guid).ok_or_else(|| anyhow!("ignore_trade: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_ignore_trade",
            gw_ignore_trade_then(self.session_actor(actor))
        )
    }
}
