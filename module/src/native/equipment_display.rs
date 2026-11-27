use lyracore_protocol::messages::vanilla::opcodes::ClientOpcodeMessage;
use lyracore_shared::constants::player_flags::{HIDE_CLOAK, HIDE_HELM};
use spacetimedb::ReducerContext;

use super::Unhandled;

/// vmangos `WorldSession::HandleShowingHelmOpcode` and `HandleShowingCloakOpcode`.
pub(super) fn receive(
    ctx: &ReducerContext,
    guid: u64,
    message: ClientOpcodeMessage,
) -> Result<(), Unhandled> {
    let flag = match message {
        ClientOpcodeMessage::CMSG_TOGGLE_HELM => HIDE_HELM,
        ClientOpcodeMessage::CMSG_TOGGLE_CLOAK => HIDE_CLOAK,
        other => return Err(Unhandled::Declined(other)),
    };
    let mut entity = super::available_entity(ctx, guid)?;
    entity.player_flags ^= flag;
    entity.update(ctx);
    Ok(())
}
