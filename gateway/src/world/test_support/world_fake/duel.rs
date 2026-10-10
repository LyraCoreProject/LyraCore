use super::super::*;

impl DuelActionStore for WorldFake {
    fn duel_accept(&self, _account_id: u64, _actor_guid: u64, _flag_guid: u64) -> Result<()> {
        Ok(())
    }

    fn duel_cancel(&self, _account_id: u64, _actor_guid: u64, _flag_guid: u64) -> Result<()> {
        Ok(())
    }
}
