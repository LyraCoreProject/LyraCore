use super::super::*;

/// Melee behaviour is tested through `InMemoryMeleeActions`. The shard routing tests read the
/// `start_attack` entry in the topology call log, so that one call stays recorded.
impl MeleeActionStore for WorldFake {
    fn start_attack(&self, _actor: Actor, target_guid: u64) -> Result<()> {
        self.rec("start_attack");
        if let Some(state) = &self.benilla_gameplay {
            state.lock().unwrap().attacking = Some(target_guid);
        }
        Ok(())
    }
    fn stop_attack(&self, _actor: Actor) -> Result<()> {
        if let Some(state) = &self.benilla_gameplay {
            state.lock().unwrap().attacking = None;
        }
        Ok(())
    }
}
