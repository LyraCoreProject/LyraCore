use super::super::*;

#[derive(Default)]
pub(crate) struct TaxiState {
    /// The taxi map the open operation returns, for the TAXI gossip socket tests.
    pub(crate) taxi_map: Option<codec::TaxiMapView>,
    pub(crate) taxi_error: Option<fn() -> anyhow::Error>,
}

/// Taxi behavior is tested through `InMemoryTaxiActions`. This adapter serves only the TAXI gossip
/// socket tests: an open that returns the configured map, or the configured failure.
impl TaxiActionStore for WorldFake {
    fn taxi_node_status(
        &self,
        _actor: Actor,
        _npc_guid: u64,
    ) -> Result<Option<codec::TaxiNodeStatusView>> {
        Ok(None)
    }

    fn open_taxi(&self, _actor: Actor, _npc_guid: u64) -> Result<Option<codec::TaxiMapView>> {
        if let Some(error) = self.taxi.taxi_error {
            return Err(error());
        }
        Ok(self.taxi.taxi_map.clone())
    }

    fn activate_taxi(
        &self,
        _actor: Actor,
        _npc_guid: u64,
        _source_client_node_id: u32,
        _destination_client_node_id: u32,
    ) -> Result<codec::TaxiActivationResult> {
        Ok(codec::TaxiActivationResult::default())
    }

    fn arm_taxi_flight(&self, _actor: Actor) -> Result<()> {
        Ok(())
    }
}
