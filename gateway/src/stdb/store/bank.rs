//! `Coordinator`'s [`BankStore`] adapter.

use anyhow::{anyhow, Result};

use crate::stdb::bindings::*;
use crate::stdb::connection::call_reducer;
use crate::stdb::Coordinator;
use crate::world::{Actor, BankStore};

impl BankStore for Coordinator {
    fn auto_bank_item(&self, account_id: u64, self_guid: u64, slot: u8) -> Result<()> {
        self.auto_bank_item(account_id, self_guid, slot)
    }

    fn buy_bank_slot(&self, account_id: u64, self_guid: u64, banker_guid: u64) -> Result<()> {
        self.buy_bank_slot(account_id, self_guid, banker_guid)
    }
}

impl Coordinator {
    /// Buy the next bank bag slot from `banker_guid` (`CMSG_BUY_BANK_SLOT`) over the coordinator
    /// connection. A refusal carries the module's `[N]` `SMSG_BUY_BANK_SLOT_RESULT` code tag.
    pub fn buy_bank_slot(&self, _account_id: u64, actor_guid: u64, banker_guid: u64) -> Result<()> {
        let actor = Actor::new(actor_guid)
            .ok_or_else(|| anyhow!("buy_bank_slot: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_buy_bank_slot",
            gw_buy_bank_slot_then(self.session_actor(actor), banker_guid)
        )
    }

    /// Auto-bank/auto-store-bank the item in `slot` (`CMSG_AUTOBANK_ITEM`/`CMSG_AUTOSTORE_BANK_ITEM`)
    /// over the coordinator connection. The module infers deposit vs. withdraw from `slot` and
    /// resolves the receiving free slot itself.
    pub fn auto_bank_item(&self, _account_id: u64, actor_guid: u64, slot: u8) -> Result<()> {
        let actor = Actor::new(actor_guid)
            .ok_or_else(|| anyhow!("auto_bank_item: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_auto_bank_item",
            gw_auto_bank_item_then(self.session_actor(actor), slot)
        )
    }
}
