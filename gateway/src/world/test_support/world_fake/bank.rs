use super::super::*;

#[derive(Default)]
pub(crate) struct BankState {
    /// Recorded `auto_bank_item` slots — backs both CMSG_AUTOBANK_ITEM (deposit) and
    /// CMSG_AUTOSTORE_BANK_ITEM (withdraw), which both route onto this one store method.
    pub(crate) auto_banked_items: std::sync::Mutex<Vec<u8>>,
    /// Recorded `buy_bank_slot` calls — the banker guid named on each `CMSG_BUY_BANK_SLOT`.
    pub(crate) bought_bank_slots: std::sync::Mutex<Vec<u64>>,
}

impl BankStore for WorldFake {
    fn auto_bank_item(&self, _account_id: u64, _self_guid: u64, slot: u8) -> Result<()> {
        if let Some(e) = &self.trade_error {
            return Err(anyhow!("{e}"));
        }
        self.bank.auto_banked_items.lock().unwrap().push(slot);
        Ok(())
    }

    fn buy_bank_slot(&self, _account_id: u64, _self_guid: u64, banker_guid: u64) -> Result<()> {
        if let Some(e) = &self.trade_error {
            return Err(anyhow!("{e}"));
        }
        self.bank
            .bought_bank_slots
            .lock()
            .unwrap()
            .push(banker_guid);
        Ok(())
    }
}
