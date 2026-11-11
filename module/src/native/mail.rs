use lyracore_protocol::{
    codec,
    messages::vanilla::{
        opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
        SMSG_SEND_MAIL_RESULT_MailResultTwo as MailResult, CMSG_SEND_MAIL, SMSG_SEND_MAIL_RESULT,
    },
};
use lyracore_shared::{faction::same_team, mail as rules};
use spacetimedb::{ConnectionId, ReducerContext, Table};

use super::Rollback;
use crate::{
    game_gameobject, game_gameobject_template, game_item_instance, game_item_template,
    mail::{self as core, game_mail, MailRefusal},
    mail_text::game_item_text,
    Refusal,
};

pub(super) fn claims(message: &ClientOpcodeMessage) -> bool {
    matches!(
        message,
        ClientOpcodeMessage::CMSG_GET_MAIL_LIST(_)
            | ClientOpcodeMessage::MSG_QUERY_NEXT_MAIL_TIME
            | ClientOpcodeMessage::CMSG_ITEM_TEXT_QUERY(_)
            | ClientOpcodeMessage::CMSG_MAIL_MARK_AS_READ(_)
            | ClientOpcodeMessage::CMSG_MAIL_DELETE(_)
            | ClientOpcodeMessage::CMSG_MAIL_RETURN_TO_SENDER(_)
            | ClientOpcodeMessage::CMSG_MAIL_TAKE_MONEY(_)
            | ClientOpcodeMessage::CMSG_MAIL_TAKE_ITEM(_)
            | ClientOpcodeMessage::CMSG_SEND_MAIL(_)
            | ClientOpcodeMessage::CMSG_MAIL_CREATE_TEXT_ITEM(_)
    )
}

pub(super) fn receive(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    message: ClientOpcodeMessage,
) -> Result<(), Rollback> {
    let actor = super::actor(ctx, guid)?;
    let mailbox = |target| at_mailbox(ctx, &actor, target);
    let run = |result: Result<(), Refusal>, reply: &dyn Fn(bool) -> SMSG_SEND_MAIL_RESULT| {
        result.map_err(|cause| Rollback::reply(response(reply(false)), cause))?;
        Ok::<_, Rollback>(response(reply(true)))
    };
    use ClientOpcodeMessage as C;
    let packet = match message {
        C::CMSG_GET_MAIL_LIST(request) => {
            mailbox(request.mailbox.guid())?;
            let mails = ctx
                .db
                .game_mail()
                .by_recipient()
                .filter(guid)
                .filter(|mail| mail.is_delivered(crate::time::now(ctx)))
                .map(|mail| view(ctx, mail))
                .collect::<Vec<_>>();
            ServerOpcodeMessage::SMSG_MAIL_LIST_RESULT(Box::new(codec::build_mail_list(
                &mails,
                crate::time::now_secs(ctx) as i64,
            )))
        }
        C::MSG_QUERY_NEXT_MAIL_TIME => ServerOpcodeMessage::MSG_QUERY_NEXT_MAIL_TIME(
            codec::build_next_mail_time(core::has_unread(ctx, guid)),
        ),
        C::CMSG_ITEM_TEXT_QUERY(request) => ServerOpcodeMessage::SMSG_ITEM_TEXT_QUERY_RESPONSE(
            Box::new(codec::build_item_text_response(
                request.item_text_id,
                item_text(ctx, guid, request.item_text_id, request.mail_id),
            )),
        ),
        C::CMSG_MAIL_MARK_AS_READ(request) => {
            mailbox(request.mailbox.guid())?;
            core::apply_mark_read(ctx, guid, request.mail_id.into())?;
            return Ok(());
        }
        C::CMSG_MAIL_DELETE(request) => run(
            mailbox(request.mailbox_id.guid())
                .and_then(|()| core::apply_delete(ctx, guid, request.mail_id.into())),
            &|ok| codec::build_mail_delete_result(request.mail_id, ok),
        )?,
        C::CMSG_MAIL_RETURN_TO_SENDER(request) => run(
            mailbox(request.mailbox_id.guid())
                .and_then(|()| core::apply_return(ctx, guid, request.mail_id.into())),
            &|ok| codec::build_mail_return_result(request.mail_id, ok),
        )?,
        C::CMSG_MAIL_TAKE_MONEY(request) => run(
            mailbox(request.mailbox.guid())
                .and_then(|()| core::apply_take_money(ctx, guid, request.mail_id.into())),
            &|ok| codec::build_mail_take_money_result(request.mail_id, ok),
        )?,
        C::CMSG_MAIL_TAKE_ITEM(request) => {
            let taken = mailbox(request.mailbox.guid())
                .map_err(MailRefusal::from)
                .and_then(|()| {
                    let mail = core::delivered_mail(ctx, request.mail_id.into())
                        .ok_or_else(|| rules::NOT_YOUR_MAIL.to_string())?;
                    core::apply_take_item(ctx, guid, mail.id)?;
                    Ok((mail.item_entry, mail.item_stack_count))
                })
                .map_err(|refusal| {
                    let error = match refusal {
                        MailRefusal::InventoryFull => codec::MailTakeItemError::BagsFull,
                        MailRefusal::CodNotAffordable | MailRefusal::NotEnoughMoney => {
                            codec::MailTakeItemError::NotEnoughMoney
                        }
                        MailRefusal::ItemIsSoulbound
                        | MailRefusal::NotYourItem
                        | MailRefusal::Refused(_) => codec::MailTakeItemError::Other,
                    };
                    Rollback::reply(
                        response(codec::build_mail_take_item_result(
                            request.mail_id,
                            Err(error),
                        )),
                        refusal,
                    )
                })?;
            response(codec::build_mail_take_item_result(
                request.mail_id,
                Ok(taken),
            ))
        }
        C::CMSG_SEND_MAIL(request) => {
            mailbox(request.mailbox.guid())?;
            send_letter(ctx, guid, *request).map_err(|refusal| {
                Rollback::reply(
                    response(codec::build_mail_send_result(refusal)),
                    "mail send refused",
                )
            })?;
            response(codec::build_mail_send_result(MailResult::Ok))
        }
        C::CMSG_MAIL_CREATE_TEXT_ITEM(request) => {
            mailbox(request.mailbox.guid())?;
            crate::mail_text::apply_copy_letter(ctx, guid, request.mail_id.into()).map_err(
                |refusal| {
                    let error = match refusal {
                        MailRefusal::InventoryFull => codec::MailMadePermanentError::BagsFull,
                        MailRefusal::NotEnoughMoney
                        | MailRefusal::CodNotAffordable
                        | MailRefusal::ItemIsSoulbound
                        | MailRefusal::NotYourItem
                        | MailRefusal::Refused(_) => codec::MailMadePermanentError::Other,
                    };
                    Rollback::reply(
                        response(codec::build_mail_made_permanent_result(
                            request.mail_id,
                            Err(error),
                        )),
                        refusal,
                    )
                },
            )?;
            response(codec::build_mail_made_permanent_result(
                request.mail_id,
                Ok(()),
            ))
        }
        _ => return Err("opcode is not a mail request".into()),
    };
    Ok(super::send(ctx, connection, packet, false)?)
}

fn response(packet: SMSG_SEND_MAIL_RESULT) -> ServerOpcodeMessage {
    ServerOpcodeMessage::SMSG_SEND_MAIL_RESULT(Box::new(packet))
}

fn at_mailbox(
    ctx: &ReducerContext,
    actor: &crate::WorldEntity,
    mailbox_guid: u64,
) -> Result<(), Refusal> {
    let in_range = ctx
        .db
        .game_gameobject()
        .guid()
        .find(mailbox_guid)
        .is_some_and(|mailbox| {
            mailbox.map_id == actor.map_id
                && mailbox.instance_id == actor.instance_id
                && ctx
                    .db
                    .game_gameobject_template()
                    .entry()
                    .find(mailbox.template_entry)
                    .is_some_and(|template| template.type_id == rules::MAILBOX_GO_TYPE)
                && (mailbox.x - actor.x).powi(2)
                    + (mailbox.y - actor.y).powi(2)
                    + (mailbox.z - actor.z).powi(2)
                    <= crate::npc_interaction::INTERACTION_RANGE_SQ
        });
    if in_range {
        Ok(())
    } else {
        Err(rules::not_at_mailbox(mailbox_guid).into())
    }
}

fn send_letter(
    ctx: &ReducerContext,
    sender_guid: u64,
    request: CMSG_SEND_MAIL,
) -> Result<(), MailResult> {
    let sender =
        crate::helpers::character_by_guid(ctx, sender_guid).ok_or(MailResult::ErrInternalError)?;
    use super::directory::game_character_profile;
    let candidates: Vec<(u64, u8)> = if crate::peer::realm_shard(ctx).is_some() {
        ctx.db
            .game_character_profile()
            .iter()
            .filter(|profile| {
                !profile.deleted && profile.name.eq_ignore_ascii_case(&request.receiver)
            })
            .map(|profile| (profile.guid, profile.race))
            .collect()
    } else {
        crate::helpers::characters_by_name(ctx, &request.receiver)
            .map(|character| (character.guid, character.race))
            .collect()
    };
    if candidates.is_empty() {
        return Err(MailResult::ErrRecipientNotFound);
    }
    if candidates
        .iter()
        .any(|character| character.0 == sender_guid)
    {
        return Err(MailResult::ErrCannotSendToSelf);
    }
    let mut reachable = candidates
        .iter()
        .filter(|character| same_team(sender.race, character.1));
    let recipient = reachable.next().ok_or(MailResult::ErrNotYourTeam)?;
    if reachable.next().is_some() {
        return Err(MailResult::ErrRecipientNotFound);
    }
    core::apply_send(
        ctx,
        sender_guid,
        recipient.0,
        request.subject,
        request.body,
        request.money.as_int(),
        rules::cod_at_send(request.cash_on_delivery_amount, request.item.guid() != 0),
        request.item.guid(),
    )
    .map_err(|refusal| match refusal {
        MailRefusal::NotEnoughMoney => MailResult::ErrNotEnoughMoney,
        MailRefusal::ItemIsSoulbound => MailResult::ErrCantSendWrappedCod,
        MailRefusal::NotYourItem => MailResult::ErrMailAttachmentInvalid,
        MailRefusal::InventoryFull | MailRefusal::CodNotAffordable | MailRefusal::Refused(_) => {
            MailResult::ErrInternalError
        }
    })
}

fn item_text(ctx: &ReducerContext, guid: u64, text_id: u32, item_hint: u32) -> String {
    let body = core::delivered_mail(ctx, text_id.into())
        .filter(|mail| mail.recipient_guid == guid)
        .map(|mail| mail.body);
    let owns_item = text_id != 0
        && item_hint != 0
        && ctx
            .db
            .game_item_instance()
            .by_owner_guid()
            .filter(guid)
            .any(|item| item.guid as u32 == item_hint && item.item_text_id == text_id);
    if body.is_none() && !owns_item {
        return String::new();
    }
    if let Some(body) = body {
        return body;
    }
    ctx.db
        .game_item_text()
        .id()
        .find(text_id)
        .map(|text| text.text)
        .unwrap_or_default()
}

fn view(ctx: &ReducerContext, mail: core::Mail) -> codec::MailView {
    codec::MailView {
        id: mail.id,
        sender_guid: mail.sender_guid,
        subject: mail.subject,
        body: mail.body,
        item_entry: mail.item_entry,
        item_stack_count: mail.item_stack_count,
        item_durability: mail.item_durability,
        max_durability: ctx
            .db
            .game_item_template()
            .entry()
            .find(mail.item_entry)
            .map_or(0, |template| template.max_durability),
        item_enchant_id: mail.item_enchant_id,
        item_soulbound: mail.item_soulbound,
        money: mail.money,
        cod: mail.cod,
        was_read: mail.was_read,
        created_at_secs: mail
            .created_at
            .to_micros_since_unix_epoch()
            .div_euclid(1_000_000),
        random_property_id: mail.random_property_id,
        sender_kind: mail.sender_kind,
        sender_entry: mail.sender_entry,
        check_flags: mail.check_flags,
        mail_template_id: mail.mail_template_id,
        deliver_secs: mail
            .deliver_micros
            .saturating_add(999_999)
            .div_euclid(1_000_000),
    }
}

pub(crate) fn arrived(ctx: &ReducerContext, recipient_guid: u64) {
    super::send_to_character(
        ctx,
        recipient_guid,
        ServerOpcodeMessage::SMSG_RECEIVED_MAIL(codec::build_received_mail()),
        super::EncodeFailure::Panic,
    );
}
