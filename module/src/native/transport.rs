use lyracore_protocol::codec;
use lyracore_protocol::messages::vanilla::SMSG_UPDATE_OBJECT;
use spacetimedb::ReducerContext;

use crate::WorldEntity;

pub(super) fn create(ctx: &ReducerContext, guid: u64) -> Result<SMSG_UPDATE_OBJECT, String> {
    let transport =
        crate::transport::find(ctx, guid).ok_or("AOI Grant refers to missing Transport")?;
    let now_ms = crate::transport::now_ms(ctx);
    let pose = crate::transport::pose(ctx, &transport, now_ms);
    let [x, y, z] = pose.map_or([0.0; 3], |pose| pose.position);
    Ok(codec::build_transport_create_object(
        &codec::TransportView {
            guid,
            entry: transport.entry,
            display_id: transport.display_id,
            size: transport.size,
            flags: transport.flags,
            x,
            y,
            z,
            orientation: pose.map_or(0.0, |pose| pose.orientation),
            path_progress_ms: crate::transport::path_progress_ms(&transport, now_ms),
        },
    ))
}

pub(super) fn passenger_movement(ctx: &ReducerContext, entity: &WorldEntity) -> Option<Vec<u8>> {
    let passenger = crate::transport::passenger(ctx, entity.guid)?;
    Some(codec::passenger_movement_bytes(
        entity.movement_flags,
        entity.last_move_ms,
        [entity.x, entity.y, entity.z, entity.orientation],
        crate::transport::guid(passenger.transport_entry),
        [passenger.x, passenger.y, passenger.z, passenger.orientation],
    ))
}
