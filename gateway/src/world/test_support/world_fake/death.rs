use super::super::*;

/// Death requests are tested against `DeathFake`. A World Session on this Fake answers
/// every one with success and no effect.
impl DeathStore for WorldFake {
    fn repop(&self, _actor: Actor) -> Result<()> {
        Ok(())
    }

    fn reclaim_corpse(&self, _actor: Actor, _corpse_guid: u64) -> Result<()> {
        Ok(())
    }

    fn resurrect_response(&self, _actor: Actor, _accept: bool) -> Result<()> {
        Ok(())
    }

    fn self_resurrect(&self, _actor: Actor) -> Result<()> {
        Ok(())
    }

    fn spirit_healer_res(&self, _actor: Actor, _healer_guid: u64) -> Result<()> {
        Ok(())
    }

    fn corpse_location(&self, _owner_guid: u64) -> Result<Option<(u32, f32, f32, f32)>> {
        Ok(None)
    }
}
