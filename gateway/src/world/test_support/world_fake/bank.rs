use super::super::*;

impl BankStore for WorldFake {
    fn auto_bank_item(&self, _actor: Actor, _slot: u8) -> Result<()> {
        Ok(())
    }

    fn buy_bank_slot(&self, _actor: Actor, _banker_guid: u64) -> Result<()> {
        Ok(())
    }
}
