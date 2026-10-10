//! `Coordinator`'s [`BankStore`] adapter.

use anyhow::Result;

use crate::stdb::bindings::*;
use crate::stdb::connection::call_reducer;
use crate::stdb::Coordinator;
use crate::world::{Actor, BankStore};

impl BankStore for Coordinator {
    /// Auto-bank/auto-store-bank the item in `slot` (`CMSG_AUTOBANK_ITEM`/`CMSG_AUTOSTORE_BANK_ITEM`)
    /// over the coordinator connection. The module infers deposit vs. withdraw from `slot` and
    /// resolves the receiving free slot itself.
    fn auto_bank_item(&self, actor: Actor, slot: u8) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_auto_bank_item",
            gw_auto_bank_item_then(self.session_actor(actor), slot)
        )
    }

    /// Buy the next bank bag slot from `banker_guid` (`CMSG_BUY_BANK_SLOT`) over the coordinator
    /// connection. A refusal carries the module's `[N]` `SMSG_BUY_BANK_SLOT_RESULT` code tag.
    fn buy_bank_slot(&self, actor: Actor, banker_guid: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_buy_bank_slot",
            gw_buy_bank_slot_then(self.session_actor(actor), banker_guid)
        )
    }
}
