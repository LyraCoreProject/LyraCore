use lyracore_protocol::{
    codec,
    messages::vanilla::{
        opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
        SMSG_SEND_MAIL_RESULT_MailResultTwo as MailResult, CMSG_SEND_MAIL, SMSG_SEND_MAIL_RESULT,
    },
};
use lyracore_shared::{faction::same_team, mail as rules};
use spacetimedb::{ConnectionId, ReducerContext};

use super::{game_native_player, Rollback};
use crate::{
    game_gameobject, game_gameobject_template, game_item_instance, game_item_template,
    mail::{self as core, game_mail},
    mail_text::game_item_text,
};

pub(super) fn receive(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    message: ClientOpcodeMessage,
) -> Result<(), Rollback> {
    let actor = super::actor(ctx, guid)?;
    let mailbox = |target| at_mailbox(ctx, &actor, target);
    let run = |result: Result<(), String>, reply: &dyn Fn(bool) -> SMSG_SEND_MAIL_RESULT| {
        result.map_err(|cause| Rollback::reply(response(reply(false)), cause))?;
        Ok::<_, Rollback>(response(reply(true)))
    };
    use ClientOpcodeMessage as C;
    let packet = match message {
        C::CMSG_GET_MAIL_LIST(request) => {
            mailbox(request.mailbox.guid()).map_err(Rollback::silent)?;
            let mails = ctx
                .db
                .game_mail()
                .by_recipient()
                .filter(guid)
                .filter(|mail| mail.is_delivered(ctx.timestamp))
                .map(|mail| view(ctx, mail))
                .collect::<Vec<_>>();
            ServerOpcodeMessage::SMSG_MAIL_LIST_RESULT(Box::new(codec::build_mail_list(
                &mails,
                ctx.timestamp
                    .to_micros_since_unix_epoch()
                    .div_euclid(1_000_000),
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
            mailbox(request.mailbox.guid())
                .and_then(|()| core::apply_mark_read(ctx, guid, request.mail_id.into()))
                .map_err(Rollback::silent)?;
            return Ok(());
        }
        C::CMSG_MAIL_DELETE(request) => run(
            mailbox(request.mailbox_id.guid())
                .and_then(|()| core::apply_delete(ctx, guid, request.mail_id.into())),
            &|ok| codec::build_mail_delete_result(request.mail_id, ok),
        )?,
        C::CMSG_MAIL_RETURN_TO_SENDER(request) => {
            let same_account = core::delivered_mail(ctx, request.mail_id.into())
                .is_some_and(|mail| same_account(ctx, guid, mail.sender_guid));
            run(
                mailbox(request.mailbox_id.guid()).and_then(|()| {
                    core::apply_return(ctx, guid, request.mail_id.into(), same_account)
                }),
                &|ok| codec::build_mail_return_result(request.mail_id, ok),
            )?
        }
        C::CMSG_MAIL_TAKE_MONEY(request) => run(
            mailbox(request.mailbox.guid())
                .and_then(|()| core::apply_take_money(ctx, guid, request.mail_id.into())),
            &|ok| codec::build_mail_take_money_result(request.mail_id, ok),
        )?,
        C::CMSG_MAIL_TAKE_ITEM(request) => {
            let taken = mailbox(request.mailbox.guid())
                .and_then(|()| {
                    let mail = core::delivered_mail(ctx, request.mail_id.into())
                        .ok_or_else(|| rules::NOT_YOUR_MAIL.to_string())?;
                    core::apply_take_item(ctx, guid, mail.id)?;
                    Ok((mail.item_entry, mail.item_stack_count))
                })
                .map_err(|cause| {
                    let refusal = if cause.contains(rules::INVENTORY_FULL) {
                        codec::MailTakeItemError::BagsFull
                    } else if cause.contains(rules::COD_NOT_AFFORDABLE)
                        || cause.contains(rules::NOT_ENOUGH_MONEY)
                    {
                        codec::MailTakeItemError::NotEnoughMoney
                    } else {
                        codec::MailTakeItemError::Other
                    };
                    Rollback::reply(
                        response(codec::build_mail_take_item_result(
                            request.mail_id,
                            Err(refusal),
                        )),
                        cause,
                    )
                })?;
            response(codec::build_mail_take_item_result(
                request.mail_id,
                Ok(taken),
            ))
        }
        C::CMSG_SEND_MAIL(request) => {
            mailbox(request.mailbox.guid()).map_err(Rollback::silent)?;
            send_letter(ctx, guid, *request).map_err(|refusal| {
                Rollback::reply(
                    response(codec::build_mail_send_result(refusal)),
                    "mail send refused",
                )
            })?;
            response(codec::build_mail_send_result(MailResult::Ok))
        }
        C::CMSG_MAIL_CREATE_TEXT_ITEM(request) => {
            mailbox(request.mailbox.guid()).map_err(Rollback::silent)?;
            crate::mail_text::apply_copy_letter(ctx, guid, request.mail_id.into()).map_err(
                |cause| {
                    let refusal = if cause.contains(rules::INVENTORY_FULL) {
                        codec::MailMadePermanentError::BagsFull
                    } else {
                        codec::MailMadePermanentError::Other
                    };
                    Rollback::reply(
                        response(codec::build_mail_made_permanent_result(
                            request.mail_id,
                            Err(refusal),
                        )),
                        cause,
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
) -> Result<(), String> {
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
                    <= rules::MAILBOX_RANGE_SQ
        });
    if in_range {
        Ok(())
    } else {
        Err(rules::not_at_mailbox(mailbox_guid))
    }
}

fn same_account(ctx: &ReducerContext, first: u64, second: u64) -> bool {
    match (
        crate::helpers::character_by_guid(ctx, first),
        crate::helpers::character_by_guid(ctx, second),
    ) {
        (Some(first), Some(second)) => {
            first.account_id != 0 && first.account_id == second.account_id
        }
        _ => false,
    }
}

fn send_letter(
    ctx: &ReducerContext,
    sender_guid: u64,
    request: CMSG_SEND_MAIL,
) -> Result<(), MailResult> {
    let sender =
        crate::helpers::character_by_guid(ctx, sender_guid).ok_or(MailResult::ErrInternalError)?;
    let candidates = crate::helpers::characters_by_name(ctx, &request.receiver).collect::<Vec<_>>();
    if candidates.is_empty() {
        return Err(MailResult::ErrRecipientNotFound);
    }
    if candidates
        .iter()
        .any(|character| character.guid == sender_guid)
    {
        return Err(MailResult::ErrCannotSendToSelf);
    }
    let mut reachable = candidates
        .iter()
        .filter(|character| same_team(sender.race, character.race));
    let recipient = reachable.next().ok_or(MailResult::ErrNotYourTeam)?;
    if reachable.next().is_some() {
        return Err(MailResult::ErrRecipientNotFound);
    }
    core::apply_send(
        ctx,
        sender_guid,
        recipient.guid,
        request.subject,
        request.body,
        request.money.as_int(),
        rules::cod_at_send(request.cash_on_delivery_amount, request.item.guid() != 0),
        request.item.guid(),
        sender.account_id != 0 && sender.account_id == recipient.account_id,
    )
    .map_err(|cause| {
        if cause.contains(rules::NOT_ENOUGH_MONEY) {
            MailResult::ErrNotEnoughMoney
        } else if cause.contains(rules::ITEM_IS_SOULBOUND) {
            MailResult::ErrCantSendWrappedCod
        } else if cause.contains(rules::NOT_YOUR_ITEM) {
            MailResult::ErrMailAttachmentInvalid
        } else {
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
    ctx.db
        .game_item_text()
        .id()
        .find(text_id)
        .map(|text| text.text)
        .or(body)
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
    if let Some(recipient) = ctx
        .db
        .game_native_player()
        .character_guid()
        .find(recipient_guid)
    {
        super::send(
            ctx,
            recipient.connection_id,
            ServerOpcodeMessage::SMSG_RECEIVED_MAIL(codec::build_received_mail()),
            false,
        )
        .expect("mail arrival fits a world frame");
    }
}
