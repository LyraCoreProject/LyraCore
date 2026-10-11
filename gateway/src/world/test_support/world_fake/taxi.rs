use super::super::*;
use std::sync::Mutex;
use wow_world_messages::vanilla::Vector3d;

#[derive(Default)]
pub(crate) struct TaxiState {
    /// The taxi map the open operation returns, for the TAXI gossip socket tests.
    pub(crate) taxi_map: Option<codec::TaxiMapView>,
    pub(crate) taxi_error: Option<fn() -> anyhow::Error>,
    pub(crate) activation: codec::TaxiActivationResult,
    pub(crate) armed_passenger: Mutex<Option<u64>>,
    pub(crate) arm_error: Option<fn() -> anyhow::Error>,
    pub(crate) flight_tx: Mutex<Option<SessionTx>>,
}

/// Taxi state for World Session tests. Arming publishes a flight spline to its passenger.
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
        Ok(self.taxi.activation)
    }

    fn arm_taxi_flight(&self, actor: Actor) -> Result<()> {
        if let Some(error) = self.taxi.arm_error {
            return Err(error());
        }
        *self.taxi.armed_passenger.lock().unwrap() = Some(actor.guid());
        if let Some(tx) = self.taxi.flight_tx.lock().unwrap().take() {
            let (opcode, body) = codec::build_taxi_move_raw(
                actor.guid(),
                Vector3d {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                },
                vec![Vector3d {
                    x: 10.0,
                    y: 0.0,
                    z: 5.0,
                }],
                1000,
                1,
            )
            .unwrap();
            send(&tx, Outbound::Raw { opcode, body })?;
        }
        Ok(())
    }
}
