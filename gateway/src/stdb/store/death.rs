//! `Coordinator`'s [`DeathStore`] adapter.

use anyhow::Result;

use crate::world::DeathStore;

use crate::stdb::Coordinator;

impl DeathStore for Coordinator {
    fn repop(&self, account_id: u64, self_guid: u64) -> Result<()> {
        self.repop(account_id, self_guid)
    }

    fn reclaim_corpse(&self, account_id: u64, self_guid: u64, corpse_guid: u64) -> Result<()> {
        self.reclaim_corpse(account_id, self_guid, corpse_guid)
    }

    fn resurrect_response(&self, account_id: u64, self_guid: u64, accept: bool) -> Result<()> {
        self.resurrect_response(account_id, self_guid, accept)
    }

    fn self_resurrect(&self, account_id: u64, self_guid: u64) -> Result<()> {
        self.self_resurrect(account_id, self_guid)
    }

    fn spirit_healer_res(&self, account_id: u64, self_guid: u64, healer_guid: u64) -> Result<()> {
        self.spirit_healer_res(account_id, self_guid, healer_guid)
    }

    fn corpse_location(&self, owner_guid: u64) -> Result<Option<(u32, f32, f32, f32)>> {
        self.corpse_location(owner_guid)
    }
}
