//! `Coordinator`'s [`BankStore`] adapter.

use anyhow::Result;

use crate::world::BankStore;

use crate::stdb::Coordinator;

impl BankStore for Coordinator {
    fn auto_bank_item(&self, account_id: u64, self_guid: u64, slot: u8) -> Result<()> {
        self.auto_bank_item(account_id, self_guid, slot)
    }

    fn buy_bank_slot(&self, account_id: u64, self_guid: u64, banker_guid: u64) -> Result<()> {
        self.buy_bank_slot(account_id, self_guid, banker_guid)
    }
}
