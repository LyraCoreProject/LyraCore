use super::super::*;

impl DuelActionStore for WorldFake {
    fn duel_accept(&self, _actor: Actor, _flag_guid: u64) -> Result<()> {
        Ok(())
    }

    fn duel_cancel(&self, _actor: Actor, _flag_guid: u64) -> Result<()> {
        Ok(())
    }
}
