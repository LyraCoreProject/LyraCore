use super::super::*;

#[derive(Default)]
pub(crate) struct MeleeState {
    /// When set, `start_attack` returns this error. Only the session-fatal desync case is driven
    /// from here now; the refusal mapping is tested on the melee seam itself.
    pub(crate) start_attack_error: Option<String>,
    /// Recorded `stop_attack` dispatches: the actor guid. The Auto Shot teardowns reach this
    /// through `WorldStore`'s `MeleeActionStore` supertrait, so it pins that resolution.
    pub(crate) stop_attacks: std::sync::Mutex<Vec<u64>>,
}

impl MeleeActionStore for WorldFake {
    fn start_attack(&self, _account_id: u64, _self_guid: u64, _target_guid: u64) -> Result<()> {
        self.rec("start_attack");
        match &self.melee.start_attack_error {
            Some(e) => Err(anyhow!("{e}")),
            None => Ok(()),
        }
    }
    fn stop_attack(&self, _account_id: u64, self_guid: u64) -> Result<()> {
        self.melee.stop_attacks.lock().unwrap().push(self_guid);
        Ok(())
    }
}
