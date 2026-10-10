use super::super::*;

/// Melee behaviour is tested through `InMemoryMeleeActions`. The shard routing tests read the
/// `start_attack` entry in the topology call log, so that one call stays recorded.
impl MeleeActionStore for WorldFake {
    fn start_attack(&self, _account_id: u64, _self_guid: u64, _target_guid: u64) -> Result<()> {
        self.rec("start_attack");
        Ok(())
    }
    fn stop_attack(&self, _account_id: u64, _self_guid: u64) -> Result<()> {
        Ok(())
    }
}
