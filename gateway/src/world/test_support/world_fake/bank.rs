use super::super::*;

impl BankStore for WorldFake {
    fn auto_bank_item(&self, _account_id: u64, _self_guid: u64, _slot: u8) -> Result<()> {
        Ok(())
    }

    fn buy_bank_slot(&self, _account_id: u64, _self_guid: u64, _banker_guid: u64) -> Result<()> {
        Ok(())
    }
}
