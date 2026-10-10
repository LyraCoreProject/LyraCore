use super::super::*;

impl TradeStore for WorldFake {
    fn initiate_trade(&self, _account_id: u64, _self_guid: u64, _target_guid: u64) -> Result<()> {
        Ok(())
    }

    fn begin_trade(&self, _account_id: u64, _self_guid: u64) -> Result<()> {
        Ok(())
    }

    fn cancel_trade(&self, _account_id: u64, _self_guid: u64) -> Result<()> {
        Ok(())
    }

    fn set_trade_item(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _trade_slot: u8,
        _inv_slot: u8,
    ) -> Result<()> {
        Ok(())
    }

    fn clear_trade_item(&self, _account_id: u64, _self_guid: u64, _trade_slot: u8) -> Result<()> {
        Ok(())
    }

    fn set_trade_gold(&self, _account_id: u64, _self_guid: u64, _copper: u32) -> Result<()> {
        Ok(())
    }

    fn accept_trade(&self, _account_id: u64, _self_guid: u64) -> Result<()> {
        Ok(())
    }

    fn unaccept_trade(&self, _account_id: u64, _self_guid: u64) -> Result<()> {
        Ok(())
    }

    fn busy_trade(&self, _account_id: u64, _self_guid: u64) -> Result<()> {
        Ok(())
    }

    fn ignore_trade(&self, _account_id: u64, _self_guid: u64) -> Result<()> {
        Ok(())
    }
}
