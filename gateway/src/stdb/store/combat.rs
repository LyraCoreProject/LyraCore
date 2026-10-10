//! `Coordinator`'s [`CombatStore`] adapter.

use anyhow::Result;

use crate::world::CombatStore;

use crate::stdb::Coordinator;

impl CombatStore for Coordinator {
    fn set_target(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()> {
        self.set_target(account_id, self_guid, target_guid)
    }

    fn pet_command(
        &self,
        account_id: u64,
        self_guid: u64,
        data: u32,
        target_guid: u64,
    ) -> Result<()> {
        self.pet_command(account_id, self_guid, data, target_guid)
    }

    fn set_sheathed(&self, account_id: u64, self_guid: u64, state: u8) -> Result<()> {
        self.set_sheathed(account_id, self_guid, state)
    }
}
