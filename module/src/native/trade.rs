use lyracore_protocol::{codec, messages::vanilla::opcodes::ClientOpcodeMessage};
use spacetimedb::ReducerContext;

use super::game_native_player;
use crate::trade::{self, TradeEvent};

pub(super) fn receive(
    ctx: &ReducerContext,
    guid: u64,
    message: ClientOpcodeMessage,
) -> Result<(), String> {
    let actor = super::actor(ctx, guid).map_err(super::rollback_silent)?;
    use ClientOpcodeMessage as C;
    match message {
        C::CMSG_INITIATE_TRADE(request) => {
            trade::apply_initiate_trade(ctx, actor, request.guid.guid())
        }
        C::CMSG_BEGIN_TRADE => trade::apply_begin_trade(ctx, actor),
        C::CMSG_CANCEL_TRADE => trade::apply_cancel_trade(ctx, actor),
        C::CMSG_SET_TRADE_ITEM(request) if request.bag == 255 => {
            trade::apply_set_trade_item(ctx, actor, request.trade_slot, request.slot)
        }
        C::CMSG_SET_TRADE_ITEM(_) => Ok(()),
        C::CMSG_CLEAR_TRADE_ITEM(request) => {
            trade::apply_clear_trade_item(ctx, actor, request.trade_slot)
        }
        C::CMSG_SET_TRADE_GOLD(request) => {
            trade::apply_set_trade_gold(ctx, actor, request.gold.as_int())
        }
        C::CMSG_ACCEPT_TRADE(_) => trade::apply_accept_trade(ctx, actor),
        C::CMSG_UNACCEPT_TRADE => trade::apply_unaccept_trade(ctx, actor),
        C::CMSG_BUSY_TRADE => trade::apply_busy_trade(ctx, actor),
        C::CMSG_IGNORE_TRADE => trade::apply_ignore_trade(ctx, actor),
        _ => return Err("opcode is not a trade request".into()),
    }
    .map_err(super::rollback_silent)
}

pub(crate) fn event(ctx: &ReducerContext, event: &TradeEvent) {
    let Some(recipient) = ctx
        .db
        .game_native_player()
        .character_guid()
        .find(event.recipient_guid)
    else {
        return;
    };
    match codec::build_trade_event(event.kind, event.other_guid, &event.payload) {
        Some(packet) => {
            super::send(ctx, recipient.connection_id, packet, false)
                .expect("bounded trade message fits a world frame");
        }
        None => spacetimedb::log::warn!(
            "trade event {} has an unknown kind or malformed offer: {}",
            event.id,
            event.kind
        ),
    }
}
