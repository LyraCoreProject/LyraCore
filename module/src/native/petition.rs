use lyracore_protocol::{
    codec,
    messages::{
        vanilla::{
            opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
            BuyResult, GuildCommand, GuildCommandResult, GuildEmblemResult,
            MSG_SAVE_GUILD_EMBLEM_Server, PetitionResult, MSG_PETITION_DECLINE,
            MSG_PETITION_RENAME, MSG_TABARDVENDOR_ACTIVATE, SMSG_BUY_FAILED,
            SMSG_TURN_IN_PETITION_RESULTS,
        },
        Guid,
    },
};
use lyracore_shared::{
    faction::team_for_race,
    guild::{event_kind, fee_kind, GuildRefusal, GUILD_CHARTER_ENTRY},
};
use spacetimedb::{ConnectionId, ReducerContext};

use super::{game_native_player, NativeWorldConnection};
use crate::{
    game_world_entity,
    guild::{
        self,
        fee::{self, GuildCharterPurchase, GuildEmblem, GuildEmblemPurchase, GuildFeeRequest},
        petition::{
            self as core, game_guild_petition, game_guild_petition_signature, GuildPetition,
            GuildPetitionOffer, GuildPetitionRename, GuildPetitionSign,
        },
        GuildEvent, GuildOp,
    },
    items::game_item_instance,
};

pub(super) fn receive(
    ctx: &ReducerContext,
    connection: NativeWorldConnection,
    guid: u64,
    message: ClientOpcodeMessage,
) -> Result<(), String> {
    let actor = crate::helpers::character_by_guid(ctx, guid)
        .ok_or_else(|| super::rollback_silent("Character is not available".into()))?;
    let id = connection.connection_id;
    use ClientOpcodeMessage as C;
    let run = |op, reply: &dyn Fn(GuildRefusal) -> Option<ServerOpcodeMessage>| {
        guild::apply_op(ctx, guid, actor.account_id, op)
            .map_err(|refusal| refused(refusal.as_tag().into(), reply))
    };
    match message {
        C::MSG_TABARDVENDOR_ACTIVATE(request) => {
            let npc = request.guid.guid();
            if fee::npc_serves(ctx, guid, npc, fee_kind::EMBLEM) {
                super::send(
                    ctx,
                    id,
                    ServerOpcodeMessage::MSG_TABARDVENDOR_ACTIVATE(MSG_TABARDVENDOR_ACTIVATE {
                        guid: Guid::new(npc),
                    }),
                    false,
                )?;
            }
        }
        C::CMSG_PETITION_SHOWLIST(request) => {
            let npc = request.guid.guid();
            if fee::npc_serves(ctx, guid, npc, fee_kind::CHARTER) {
                super::send(
                    ctx,
                    id,
                    ServerOpcodeMessage::SMSG_PETITION_SHOWLIST(Box::new(
                        codec::build_petition_showlist(npc),
                    )),
                    false,
                )?;
            }
        }
        C::MSG_SAVE_GUILD_EMBLEM(request) => {
            fee::purchase(
                ctx,
                guid,
                GuildFeeRequest::Emblem(GuildEmblemPurchase {
                    npc_guid: request.vendor.guid(),
                    emblem: GuildEmblem {
                        emblem_style: request.emblem_style,
                        emblem_color: request.emblem_color,
                        border_style: request.border_style,
                        border_color: request.border_color,
                        background_color: request.background_color,
                    },
                }),
            )
            .map_err(|cause| {
                refused(cause, &|refusal| {
                    Some(emblem_result(match refusal {
                        GuildRefusal::NotInGuild => GuildEmblemResult::NoGuild,
                        GuildRefusal::NotLeader => GuildEmblemResult::NotGuildMaster,
                        GuildRefusal::NotEnoughMoney => GuildEmblemResult::NotEnoughMoney,
                        _ => GuildEmblemResult::NoMessage,
                    }))
                })
            })?;
            super::send(ctx, id, emblem_result(GuildEmblemResult::Success), false)?;
        }
        C::CMSG_PETITION_BUY(request) => {
            let npc = request.npc.guid();
            let name = request.name;
            fee::purchase(
                ctx,
                guid,
                GuildFeeRequest::Charter(GuildCharterPurchase {
                    npc_guid: npc,
                    name: name.clone(),
                }),
            )
            .map_err(|cause| refused(cause, &|refusal| purchase_refusal(refusal, npc, &name)))?;
        }
        C::CMSG_PETITION_QUERY(request) => {
            if let Some(petition) = view(ctx, request.petition.guid()) {
                super::send(ctx, id, query(&petition), false)?;
            }
        }
        C::CMSG_PETITION_SHOW_SIGNATURES(request) => {
            if guild::member(ctx, guid).is_none() {
                if let Some(petition) = view(ctx, request.item.guid()) {
                    super::send(ctx, id, signatures(&petition), false)?;
                }
            }
        }
        C::MSG_PETITION_RENAME(request) => {
            let charter = request.petition.guid();
            if !core::holds_charter(ctx, guid, charter) {
                return Ok(());
            }
            let name = request.new_name;
            run(
                GuildOp::RenamePetition(GuildPetitionRename {
                    charter_item_guid: charter,
                    name: name.clone(),
                }),
                &|refusal| name_refusal(refusal, &name),
            )?;
            super::send(
                ctx,
                id,
                ServerOpcodeMessage::MSG_PETITION_RENAME(Box::new(MSG_PETITION_RENAME {
                    petition: Guid::new(charter),
                    new_name: name,
                })),
                false,
            )?;
        }
        C::CMSG_OFFER_PETITION(request) => {
            let charter = request.petition.guid();
            let Some(target) = crate::helpers::character_by_guid(ctx, request.target.guid())
                .filter(|character| {
                    ctx.db
                        .game_world_entity()
                        .guid()
                        .find(character.guid)
                        .is_some()
                })
            else {
                return Ok(());
            };
            if !core::holds_charter(ctx, guid, charter) {
                return Ok(());
            }
            run(
                GuildOp::OfferPetition(GuildPetitionOffer {
                    charter_item_guid: charter,
                    target_guid: target.guid,
                    target_team: team_for_race(target.race),
                }),
                &|refusal| standing_refusal(refusal, &target.name),
            )?;
        }
        C::CMSG_PETITION_SIGN(request) => {
            let charter = request.petition.guid();
            run(
                GuildOp::SignPetition(GuildPetitionSign {
                    charter_item_guid: charter,
                    actor_name: actor.name.clone(),
                    actor_team: team_for_race(actor.race),
                }),
                &|refusal| match refusal {
                    GuildRefusal::CantSignOwn => {
                        Some(sign_result(charter, guid, PetitionResult::CantSignOwn))
                    }
                    _ => standing_refusal(refusal, &actor.name),
                },
            )?;
        }
        C::MSG_PETITION_DECLINE(request) => {
            run(
                GuildOp::DeclinePetition {
                    charter_item_guid: request.petition.guid(),
                },
                &|_| None,
            )?;
        }
        C::CMSG_TURN_IN_PETITION(request) => {
            let charter = request.petition.guid();
            if !core::holds_charter(ctx, guid, charter) {
                return Ok(());
            }
            let name = view(ctx, charter).map_or(String::new(), |petition| petition.name);
            run(
                GuildOp::TurnInPetition {
                    charter_item_guid: charter,
                },
                &|refusal| match refusal {
                    GuildRefusal::AlreadyInGuild => {
                        Some(turn_in_result(PetitionResult::AlreadyInGuild))
                    }
                    GuildRefusal::NeedMoreSignatures => {
                        Some(turn_in_result(PetitionResult::NeedMore))
                    }
                    GuildRefusal::NameExists => name_refusal(refusal, &name),
                    _ => None,
                },
            )?;
            crate::ItemInstance::delete(ctx, charter);
            super::send(
                ctx,
                id,
                command(
                    GuildCommand::Create,
                    &name,
                    GuildCommandResult::PlayerNoMoreInGuild,
                ),
                false,
            )?;
            super::send(ctx, id, turn_in_result(PetitionResult::Ok), false)?;
        }
        _ => return Err("opcode is not a guild purchase or petition request".into()),
    }
    Ok(())
}

fn refused(cause: String, reply: &dyn Fn(GuildRefusal) -> Option<ServerOpcodeMessage>) -> String {
    let Some(refusal) = GuildRefusal::parse_tag(&cause) else {
        return cause;
    };
    match reply(refusal) {
        Some(packet) => super::rollback_reply(packet, cause),
        None => super::rollback_silent(cause),
    }
}

fn command(command: GuildCommand, name: &str, result: GuildCommandResult) -> ServerOpcodeMessage {
    ServerOpcodeMessage::SMSG_GUILD_COMMAND_RESULT(Box::new(codec::build_guild_command_result(
        command,
        name.into(),
        result,
    )))
}

fn emblem_result(result: GuildEmblemResult) -> ServerOpcodeMessage {
    ServerOpcodeMessage::MSG_SAVE_GUILD_EMBLEM(MSG_SAVE_GUILD_EMBLEM_Server { result })
}

fn turn_in_result(result: PetitionResult) -> ServerOpcodeMessage {
    ServerOpcodeMessage::SMSG_TURN_IN_PETITION_RESULTS(SMSG_TURN_IN_PETITION_RESULTS { result })
}

fn sign_result(charter: u64, signer: u64, result: PetitionResult) -> ServerOpcodeMessage {
    ServerOpcodeMessage::SMSG_PETITION_SIGN_RESULTS(Box::new(codec::build_petition_sign_results(
        charter, signer, result,
    )))
}

fn signatures(petition: &codec::PetitionView) -> ServerOpcodeMessage {
    ServerOpcodeMessage::SMSG_PETITION_SHOW_SIGNATURES(Box::new(
        codec::build_petition_show_signatures(petition),
    ))
}

fn query(petition: &codec::PetitionView) -> ServerOpcodeMessage {
    ServerOpcodeMessage::SMSG_PETITION_QUERY_RESPONSE(Box::new(
        codec::build_petition_query_response(petition),
    ))
}

fn standing_refusal(refusal: GuildRefusal, name: &str) -> Option<ServerOpcodeMessage> {
    let (command_kind, name, result) = match refusal {
        GuildRefusal::NotAllied => (GuildCommand::Create, "", GuildCommandResult::GuildNotAllied),
        GuildRefusal::AlreadyInGuild => (
            GuildCommand::Invite,
            name,
            GuildCommandResult::AlreadyInGuildS,
        ),
        GuildRefusal::AlreadyInvited => (
            GuildCommand::Invite,
            name,
            GuildCommandResult::AlreadyInvitedToGuildS,
        ),
        _ => return None,
    };
    Some(command(command_kind, name, result))
}

fn name_refusal(refusal: GuildRefusal, name: &str) -> Option<ServerOpcodeMessage> {
    Some(command(
        GuildCommand::Create,
        name,
        match refusal {
            GuildRefusal::NameExists => GuildCommandResult::GuildNameExistsS,
            GuildRefusal::NameInvalid => GuildCommandResult::GuildNameInvalid,
            _ => return None,
        },
    ))
}

fn purchase_refusal(refusal: GuildRefusal, npc: u64, name: &str) -> Option<ServerOpcodeMessage> {
    Some(match refusal {
        GuildRefusal::NotEnoughMoney | GuildRefusal::CharterUnavailable => {
            let (guid, result) = if refusal == GuildRefusal::NotEnoughMoney {
                (npc, BuyResult::NotEnoughMoney)
            } else {
                (0, BuyResult::CantFindItem)
            };
            ServerOpcodeMessage::SMSG_BUY_FAILED(Box::new(SMSG_BUY_FAILED {
                guid: Guid::new(guid),
                item: GUILD_CHARTER_ENTRY,
                result,
            }))
        }
        GuildRefusal::BagsFull => ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(Box::new(
            codec::build_inventory_refusal(lyracore_shared::item::ItemRefusal::InventoryFull, 0, 0),
        )),
        GuildRefusal::CharterLimit => ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(Box::new(
            codec::build_cant_carry_more_of_this(),
        )),
        _ => return name_refusal(refusal, name),
    })
}

fn view(ctx: &ReducerContext, charter: u64) -> Option<codec::PetitionView> {
    let petition = ctx
        .db
        .game_guild_petition()
        .charter_item_guid()
        .find(charter)?;
    Some(codec::PetitionView {
        petition_id: petition.petition_id,
        charter_item_guid: charter,
        owner_guid: petition.owner_guid,
        name: petition.name,
        signers: ctx
            .db
            .game_guild_petition_signature()
            .by_petition()
            .filter(petition.petition_id)
            .map(|signature| signature.signer_guid)
            .collect(),
    })
}

pub(crate) fn event(ctx: &ReducerContext, event: &GuildEvent) {
    let Some(recipient) = ctx
        .db
        .game_native_player()
        .character_guid()
        .find(event.recipient_guid)
    else {
        return;
    };
    let packet = match event.kind {
        event_kind::PETITION_OFFERED => {
            view(ctx, event.other_guid).map(|petition| signatures(&petition))
        }
        event_kind::PETITION_CHANGED => {
            view(ctx, event.other_guid).map(|petition| query(&petition))
        }
        event_kind::PETITION_SIGNED => Some(sign_result(
            event.other_guid,
            event.subject_guid,
            PetitionResult::Ok,
        )),
        event_kind::PETITION_ALREADY_SIGNED => Some(sign_result(
            event.other_guid,
            event.subject_guid,
            PetitionResult::AlreadySigned,
        )),
        event_kind::PETITION_DECLINED => Some(ServerOpcodeMessage::MSG_PETITION_DECLINE(
            MSG_PETITION_DECLINE {
                petition: Guid::new(event.subject_guid),
            },
        )),
        event_kind::FOUNDER => Some(command(
            GuildCommand::Founder,
            event.strings.first().map_or("", String::as_str),
            GuildCommandResult::PlayerNoMoreInGuild,
        )),
        _ => None,
    };
    if let Some(packet) = packet {
        super::send(ctx, recipient.connection_id, packet, false)
            .expect("bounded petition message fits a world frame");
    }
}

pub(crate) fn charter_opened(ctx: &ReducerContext, petition: &GuildPetition) {
    if let Some(owner) = ctx
        .db
        .game_native_player()
        .character_guid()
        .find(petition.owner_guid)
    {
        charter_values(
            ctx,
            owner.connection_id,
            petition.charter_item_guid,
            petition.petition_id,
        );
    }
}

fn charter_values(ctx: &ReducerContext, connection: ConnectionId, item: u64, petition: u32) {
    let (opcode, body) = codec::build_charter_petition_values(item, petition);
    super::send_raw(ctx, connection, opcode, body, false);
}

pub(super) fn replay(ctx: &ReducerContext, guid: u64, connection: ConnectionId) {
    let petition = ctx.db.game_guild_petition().owner_guid().find(guid);
    let fee_pending = fee::character_has_fee_hold(ctx, guid);
    for item in ctx
        .db
        .game_item_instance()
        .by_owner_guid()
        .filter(guid)
        .filter(|item| item.entry == GUILD_CHARTER_ENTRY)
        .collect::<Vec<_>>()
    {
        if let Some(petition) = petition
            .as_ref()
            .filter(|petition| petition.charter_item_guid == item.guid)
        {
            charter_values(ctx, connection, item.guid, petition.petition_id);
        } else if !fee_pending {
            crate::ItemInstance::delete(ctx, item.guid);
        }
    }
}
