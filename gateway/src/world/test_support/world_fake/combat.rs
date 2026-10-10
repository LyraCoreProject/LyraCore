use super::super::*;
use crate::stdb::ReducerCallError;

#[derive(Default)]
pub(crate) struct CombatState {
    /// When set, `set_target` fails as a Transport Loss before the reducer can complete. This
    /// models the call pipe whose transport dies while an admitted World Session is in flight.
    pub(crate) set_target_transport_lost: bool,
}

impl CombatStore for WorldFake {
    fn set_target(&self, _actor: Actor, target_guid: u64) -> Result<()> {
        if self.combat.set_target_transport_lost {
            return Err(ReducerCallError::transport_lost("gw_set_target").into());
        }
        if let Some(state) = &self.benilla_gameplay {
            state.lock().unwrap().selected = target_guid;
        }
        Ok(())
    }

    fn pet_command(&self, _actor: Actor, _data: u32, _target_guid: u64) -> Result<()> {
        Ok(())
    }

    fn set_sheathed(&self, _actor: Actor, _state: u8) -> Result<()> {
        Ok(())
    }
}
