//! Death family: Release Spirit, corpse reclaim and the resurrection paths.

use super::super::*;

/// Release Spirit, corpse reclaim and resurrection.
pub(crate) trait DeathStore: Send + Sync {
    /// Revive the caller after death (`CMSG_REPOP_REQUEST` / Release Spirit): the module
    /// restores full health in place and clears the dead state (the client leaves the death screen
    /// once the restored health replicates).
    fn repop(&self, account_id: u64, self_guid: u64) -> Result<()>;

    /// Reclaim the caller's corpse (`CMSG_RECLAIM_CORPSE`): the module validates the caller
    /// is a ghost owning the corpse, in range, past the reclaim delay, then resurrects at 50%.
    fn reclaim_corpse(&self, account_id: u64, self_guid: u64, corpse_guid: u64) -> Result<()>;

    /// Answer a pending resurrect offer (`CMSG_RESURRECT_RESPONSE`): `accept=true` revives the
    /// caller at the offer's frozen `%`; either way the offer is consumed. A failure (no pending offer
    /// for the caller) is expected when the offer already lapsed/was answered — per-action, log + ignore.
    fn resurrect_response(&self, account_id: u64, self_guid: u64, accept: bool) -> Result<()>;

    /// Use the caller's Self-Resurrection Option (`CMSG_SELF_RES`): the module revives the dead
    /// caller in place and spends the option. A Refusal (alive, or no option) is expected after a
    /// race. Per-action: log and ignore.
    fn self_resurrect(&self, account_id: u64, self_guid: u64) -> Result<()>;

    /// Spirit-Healer resurrect (`CMSG_SPIRIT_HEALER_ACTIVATE`): a ghost activates the graveyard Spirit
    /// Healer to res IN PLACE at 50% health/mana + a Resurrection Sickness debuff. `healer_guid` is the
    /// activated healer's guid (passed through to the confirm echo). The module gates on ghost state.
    fn spirit_healer_res(&self, account_id: u64, self_guid: u64, healer_guid: u64) -> Result<()>;

    /// Find `owner_guid`'s corpse location `(map_id, x, y, z)` for `MSG_CORPSE_QUERY`.
    fn corpse_location(&self, owner_guid: u64) -> Result<Option<(u32, f32, f32, f32)>>;
}
