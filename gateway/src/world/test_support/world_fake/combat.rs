use super::super::*;

#[derive(Default)]
pub(crate) struct CombatState {
    /// When set, `set_target` fails before the reducer can complete. This models the call pipe
    /// whose transport dies while an admitted world session is in flight.
    pub(crate) set_target_error: Option<String>,
}

impl CombatStore for WorldFake {
    fn set_target(&self, _account_id: u64, _self_guid: u64, _target_guid: u64) -> Result<()> {
        match &self.combat.set_target_error {
            Some(e) => Err(anyhow!("{e}")),
            None => Ok(()),
        }
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

    fn set_sheathed(&self, _account_id: u64, _self_guid: u64, _state: u8) -> Result<()> {
        Ok(())
    }
}
