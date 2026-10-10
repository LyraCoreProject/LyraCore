//! `Coordinator`'s [`TradeStore`] adapter.

use anyhow::Result;

use crate::world::TradeStore;

use crate::stdb::Coordinator;

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
