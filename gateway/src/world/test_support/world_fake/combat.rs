use super::super::*;

#[derive(Default)]
pub(crate) struct CombatState {
    /// Recorded `set_sheathed` dispatches: (self_guid, state), the `CMSG_SETSHEATHED` route.
    pub(crate) sheathed: std::sync::Mutex<Vec<(u64, u8)>>,
    /// Recorded `set_target` target guids — CMSG_SET_SELECTION. (`rec("set_target")` already
    /// pins the per-shard call NAME; this pins the ARGUMENT actually threaded through.)
    pub(crate) selected_targets: std::sync::Mutex<Vec<u64>>,
    /// When set, `set_target` fails before the reducer can complete. This models the call pipe
    /// whose transport dies while an admitted world session is in flight.
    pub(crate) set_target_error: Option<String>,
}

impl CombatStore for WorldFake {
    fn set_target(&self, _account_id: u64, _self_guid: u64, target_guid: u64) -> Result<()> {
        self.rec("set_target");
        if let Some(e) = &self.combat.set_target_error {
            return Err(anyhow!("{e}"));
        }
        self.combat
            .selected_targets
            .lock()
            .unwrap()
            .push(target_guid);
        Ok(())
    }

    fn pet_command(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _data: u32,
        _target_guid: u64,
    ) -> Result<()> {
        Ok(())
    }

    fn set_sheathed(&self, _account_id: u64, self_guid: u64, state: u8) -> Result<()> {
        self.combat
            .sheathed
            .lock()
            .unwrap()
            .push((self_guid, state));
        Ok(())
    }
}
