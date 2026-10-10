use super::super::*;

#[derive(Default)]
pub(crate) struct TradeState {
    /// Recorded `initiate_trade` calls, `(self_guid, target_guid)` off CMSG_INITIATE_TRADE.
    pub(crate) initiated_trades: std::sync::Mutex<Vec<(u64, u64)>>,
    /// Recorded `begin_trade` self_guids, CMSG_BEGIN_TRADE.
    pub(crate) begun_trades: std::sync::Mutex<Vec<u64>>,
    /// Recorded `cancel_trade` self_guids, CMSG_CANCEL_TRADE.
    pub(crate) cancelled_trades: std::sync::Mutex<Vec<u64>>,
    /// Recorded `set_trade_item` calls — `(self_guid, trade_slot, inv_slot)` AFTER the gateway's
    /// (bag, slot) → absolute-slot mapping.
    pub(crate) set_trade_items: std::sync::Mutex<Vec<(u64, u8, u8)>>,
    /// Recorded `clear_trade_item` calls, `(self_guid, trade_slot)`.
    pub(crate) cleared_trade_items: std::sync::Mutex<Vec<(u64, u8)>>,
    /// Recorded `set_trade_gold` calls, `(self_guid, copper)` after the wire's Gold decode.
    pub(crate) set_trade_golds: std::sync::Mutex<Vec<(u64, u32)>>,
    /// Recorded `accept_trade` self_guids, CMSG_ACCEPT_TRADE.
    pub(crate) accepted_trades: std::sync::Mutex<Vec<u64>>,
    /// Recorded `unaccept_trade` self_guids, CMSG_UNACCEPT_TRADE.
    pub(crate) unaccepted_trades: std::sync::Mutex<Vec<u64>>,
    /// Recorded `busy_trade` self_guids, CMSG_BUSY_TRADE.
    pub(crate) busy_trades: std::sync::Mutex<Vec<u64>>,
    /// Recorded `ignore_trade` self_guids, CMSG_IGNORE_TRADE.
    pub(crate) ignore_trades: std::sync::Mutex<Vec<u64>>,
}

impl TradeStore for WorldFake {
    // Trade: pure recorders, the module owns every gate, so the fake just proves which
    // verb the dispatch chose and which args survived the wire.
    fn initiate_trade(&self, _account_id: u64, self_guid: u64, target_guid: u64) -> Result<()> {
        self.rec("initiate_trade");
        self.trade
            .initiated_trades
            .lock()
            .unwrap()
            .push((self_guid, target_guid));
        Ok(())
    }

    fn begin_trade(&self, _account_id: u64, self_guid: u64) -> Result<()> {
        self.rec("begin_trade");
        self.trade.begun_trades.lock().unwrap().push(self_guid);
        Ok(())
    }

    fn cancel_trade(&self, _account_id: u64, self_guid: u64) -> Result<()> {
        self.rec("cancel_trade");
        self.trade.cancelled_trades.lock().unwrap().push(self_guid);
        Ok(())
    }

    fn set_trade_item(
        &self,
        _account_id: u64,
        self_guid: u64,
        trade_slot: u8,
        inv_slot: u8,
    ) -> Result<()> {
        self.rec("set_trade_item");
        self.trade
            .set_trade_items
            .lock()
            .unwrap()
            .push((self_guid, trade_slot, inv_slot));
        Ok(())
    }

    fn clear_trade_item(&self, _account_id: u64, self_guid: u64, trade_slot: u8) -> Result<()> {
        self.rec("clear_trade_item");
        self.trade
            .cleared_trade_items
            .lock()
            .unwrap()
            .push((self_guid, trade_slot));
        Ok(())
    }

    fn set_trade_gold(&self, _account_id: u64, self_guid: u64, copper: u32) -> Result<()> {
        self.rec("set_trade_gold");
        self.trade
            .set_trade_golds
            .lock()
            .unwrap()
            .push((self_guid, copper));
        Ok(())
    }

    fn accept_trade(&self, _account_id: u64, self_guid: u64) -> Result<()> {
        self.rec("accept_trade");
        self.trade.accepted_trades.lock().unwrap().push(self_guid);
        Ok(())
    }

    fn unaccept_trade(&self, _account_id: u64, self_guid: u64) -> Result<()> {
        self.rec("unaccept_trade");
        self.trade.unaccepted_trades.lock().unwrap().push(self_guid);
        Ok(())
    }

    fn busy_trade(&self, _account_id: u64, self_guid: u64) -> Result<()> {
        self.rec("busy_trade");
        self.trade.busy_trades.lock().unwrap().push(self_guid);
        Ok(())
    }

    fn ignore_trade(&self, _account_id: u64, self_guid: u64) -> Result<()> {
        self.rec("ignore_trade");
        self.trade.ignore_trades.lock().unwrap().push(self_guid);
        Ok(())
    }
}
