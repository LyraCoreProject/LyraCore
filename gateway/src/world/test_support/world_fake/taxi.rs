use super::super::*;

#[derive(Default)]
pub(crate) struct TaxiState {
    /// Taxi seam fixtures and operation log. Both direct query opcodes and TAXI gossip must append
    /// the same `open` operation here.
    pub(crate) taxi_status: Option<codec::TaxiNodeStatusView>,
    pub(crate) taxi_map: Option<codec::TaxiMapView>,
    pub(crate) taxi_activation: codec::TaxiActivationResult,
    pub(crate) taxi_activation_inputs: std::sync::Mutex<Vec<(u64, u64, u32, u32)>>,
    pub(crate) taxi_error: Option<String>,
    pub(crate) taxi_calls: std::sync::Mutex<Vec<(&'static str, u64, u64)>>,
}

/// Taxi behavior has focused handler tests. The broad encrypted-session store opts out unless a
/// socket test explicitly needs a taxi reply.
impl TaxiActionStore for WorldFake {
    fn taxi_node_status(
        &self,
        character_guid: u64,
        npc_guid: u64,
    ) -> Result<Option<codec::TaxiNodeStatusView>> {
        self.taxi
            .taxi_calls
            .lock()
            .unwrap()
            .push(("status", character_guid, npc_guid));
        if let Some(error) = &self.taxi.taxi_error {
            return Err(anyhow!("{error}"));
        }
        Ok(self.taxi.taxi_status)
    }

    fn open_taxi(&self, character_guid: u64, npc_guid: u64) -> Result<Option<codec::TaxiMapView>> {
        self.taxi
            .taxi_calls
            .lock()
            .unwrap()
            .push(("open", character_guid, npc_guid));
        if let Some(error) = &self.taxi.taxi_error {
            return Err(anyhow!("{error}"));
        }
        Ok(self.taxi.taxi_map.clone())
    }

    fn activate_taxi(
        &self,
        character_guid: u64,
        npc_guid: u64,
        source_client_node_id: u32,
        destination_client_node_id: u32,
    ) -> Result<codec::TaxiActivationResult> {
        self.taxi
            .taxi_calls
            .lock()
            .unwrap()
            .push(("activate", character_guid, npc_guid));
        self.taxi.taxi_activation_inputs.lock().unwrap().push((
            character_guid,
            npc_guid,
            source_client_node_id,
            destination_client_node_id,
        ));
        if let Some(error) = &self.taxi.taxi_error {
            return Err(anyhow!("{error}"));
        }
        Ok(self.taxi.taxi_activation)
    }

    fn arm_taxi_flight(&self, character_guid: u64) -> Result<()> {
        self.taxi
            .taxi_calls
            .lock()
            .unwrap()
            .push(("arm", character_guid, 0));
        Ok(())
    }
}
