use super::super::*;

/// Death requests are tested against `HandleLootFake`. A World Session on this Fake answers
/// every one with success and no effect.
impl DeathStore for WorldFake {
    fn repop(&self, _account_id: u64, _self_guid: u64) -> Result<()> {
        Ok(())
    }

    fn reclaim_corpse(&self, _account_id: u64, _self_guid: u64, _corpse_guid: u64) -> Result<()> {
        Ok(())
    }

    fn resurrect_response(&self, _account_id: u64, _self_guid: u64, _accept: bool) -> Result<()> {
        Ok(())
    }

    fn self_resurrect(&self, _account_id: u64, _self_guid: u64) -> Result<()> {
        Ok(())
    }

    fn spirit_healer_res(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _healer_guid: u64,
    ) -> Result<()> {
        Ok(())
    }

    fn corpse_location(&self, _owner_guid: u64) -> Result<Option<(u32, f32, f32, f32)>> {
        Ok(None)
    }
}
