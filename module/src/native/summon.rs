use lyracore_protocol::{codec, messages::vanilla::opcodes::ClientOpcodeMessage};
use spacetimedb::ReducerContext;

use super::Unhandled;
use crate::spell::CastRefusalReason;
use crate::summoning_ritual::{SummonOffer, SUMMON_OFFER_MICROS};

pub(super) fn receive(
    ctx: &ReducerContext,
    guid: u64,
    message: ClientOpcodeMessage,
) -> Result<(), Unhandled> {
    match message {
        // Build 5875 sends this only to accept; a declined offer lapses unanswered.
        ClientOpcodeMessage::CMSG_SUMMON_RESPONSE(request) => {
            super::actor(ctx, guid)?;
            Ok(crate::summoning_ritual::accept(
                ctx,
                guid,
                request.summoner.guid(),
            )?)
        }
        other => Err(Unhandled::Declined(other)),
    }
}

pub(crate) fn cast_refused(
    ctx: &ReducerContext,
    guid: u64,
    spell_id: u32,
    reason: CastRefusalReason,
) {
    super::send_raw_to_character(
        ctx,
        guid,
        codec::SMSG_CAST_RESULT_OPCODE,
        codec::build_cast_result_failed(spell_id, super::cast::cast_result(reason)),
    );
}

pub(crate) fn offered(ctx: &ReducerContext, offer: &SummonOffer, zone_id: u32) {
    let (opcode, body) = codec::build_summon_request_raw(
        offer.summoner_guid,
        zone_id,
        std::time::Duration::from_micros(SUMMON_OFFER_MICROS as u64),
    );
    super::send_raw_to_character(ctx, offer.target_guid, opcode, body);
}
