use super::super::*;

#[derive(Default)]
pub(crate) struct DeathState {
    /// Recorded `repop` calls — the caller's self_guid, one entry per CMSG_REPOP_REQUEST.
    pub(crate) repopped: std::sync::Mutex<Vec<u64>>,
    /// Recorded `reclaim_corpse` calls — `(self_guid, corpse_guid)` off CMSG_RECLAIM_CORPSE.
    pub(crate) reclaimed_corpses: std::sync::Mutex<Vec<(u64, u64)>>,
    /// Recorded `resurrect_response` calls — `(self_guid, accept)`, pinning the wire's
    /// `status != 0` → bool mapping.
    pub(crate) resurrect_responses: std::sync::Mutex<Vec<(u64, bool)>>,
    /// Recorded `self_resurrect` calls: the caller's self_guid off CMSG_SELF_RES.
    pub(crate) self_resurrects: std::sync::Mutex<Vec<u64>>,
    /// When set, `self_resurrect` returns this Refusal after recording the call.
    pub(crate) self_resurrect_error: Option<String>,
    /// Recorded `spirit_healer_res` calls — `(self_guid, healer_guid)` off
    /// CMSG_SPIRIT_HEALER_ACTIVATE.
    pub(crate) spirit_healer_calls: std::sync::Mutex<Vec<(u64, u64)>>,
}

impl DeathStore for WorldFake {
    fn repop(&self, _account_id: u64, self_guid: u64) -> Result<()> {
        self.death.repopped.lock().unwrap().push(self_guid);
        Ok(())
    }

    fn reclaim_corpse(&self, _account_id: u64, self_guid: u64, corpse_guid: u64) -> Result<()> {
        self.death
            .reclaimed_corpses
            .lock()
            .unwrap()
            .push((self_guid, corpse_guid));
        Ok(())
    }

    fn resurrect_response(&self, _account_id: u64, self_guid: u64, accept: bool) -> Result<()> {
        self.death
            .resurrect_responses
            .lock()
            .unwrap()
            .push((self_guid, accept));
        Ok(())
    }

    fn self_resurrect(&self, _account_id: u64, self_guid: u64) -> Result<()> {
        self.death.self_resurrects.lock().unwrap().push(self_guid);
        match &self.death.self_resurrect_error {
            Some(e) => Err(anyhow!("{e}")),
            None => Ok(()),
        }
    }

    fn spirit_healer_res(&self, _account_id: u64, self_guid: u64, healer_guid: u64) -> Result<()> {
        self.death
            .spirit_healer_calls
            .lock()
            .unwrap()
            .push((self_guid, healer_guid));
        Ok(())
    }

    fn corpse_location(&self, _owner_guid: u64) -> Result<Option<(u32, f32, f32, f32)>> {
        Ok(None)
    }
}
