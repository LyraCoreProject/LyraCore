use lyracore_protocol::messages::vanilla::opcodes::ClientOpcodeMessage;
use spacetimedb::ReducerContext;

use super::Unhandled;

pub(super) fn receive(
    ctx: &ReducerContext,
    guid: u64,
    message: ClientOpcodeMessage,
) -> Result<(), Unhandled> {
    let ClientOpcodeMessage::CMSG_TOGGLE_PVP(request) = message else {
        return Err(Unhandled::Declined(message));
    };
    super::available_entity(ctx, guid)?;
    crate::pvp::toggle(ctx, guid, request.set.map(|set| set.enable_pvp));
    Ok(())
}
