use lyracore_protocol::{codec, messages::vanilla::opcodes::ServerOpcodeMessage};
use spacetimedb::{ReducerContext, Table};

use crate::gameobject::{game_gameobject_template, GameObject};

pub(super) fn view(
    ctx: &ReducerContext,
    object: &GameObject,
) -> Result<codec::GameObjectView, String> {
    let template = ctx
        .db
        .game_gameobject_template()
        .entry()
        .find(object.template_entry)
        .ok_or("GameObject has no template")?;
    Ok(codec::GameObjectView {
        guid: object.guid,
        template_entry: object.template_entry,
        x: object.x,
        y: object.y,
        z: object.z,
        orientation: object.orientation,
        state: object.state,
        type_id: template.type_id,
        display_id: template.display_id,
        rotation_0: object.rotation_0,
        rotation_1: object.rotation_1,
        rotation_2: object.rotation_2,
        rotation_3: object.rotation_3,
        size: template.size,
    })
}

fn projected(object: &GameObject) -> (u32, u8, [f32; 8]) {
    (
        object.template_entry,
        object.state,
        [
            object.x,
            object.y,
            object.z,
            object.orientation,
            object.rotation_0,
            object.rotation_1,
            object.rotation_2,
            object.rotation_3,
        ],
    )
}

pub(crate) fn changed(ctx: &ReducerContext, old: &GameObject, object: &GameObject) {
    use super::game_native_player;
    if projected(old) == projected(object) || ctx.db.game_native_player().count() == 0 {
        return;
    }
    let Ok(view) = view(ctx, object) else {
        spacetimedb::log::warn!("GameObject {} has no template", object.guid);
        return;
    };
    let packet = lyracore_protocol::world::encode(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
        Box::new(codec::build_gameobject_values(&view)),
    ))
    .expect("GameObject fields fit a world frame");
    let rotation = codec::build_gameobject_rotation_values(&view);
    for (opcode, body) in [(packet.opcode, packet.body), rotation] {
        super::visibility::broadcast_aoi(
            ctx,
            super::NativeAoi {
                layer: crate::aoi::GAME_OBJECT,
                entity_guid: object.guid,
                exclude_connection: None,
            },
            opcode,
            body,
        );
    }
}
