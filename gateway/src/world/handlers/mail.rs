//! Mail family: open the mailbox window, answer the client's mail poll, serve a letter's body, mark
//! a mail read or delete it, post a letter with copper and one item attached, and take either back
//! out.
//!
//! Return-to-sender and COD are later slices, and their opcodes fall through to the next handler
//! until they land.
//!
//! Every failure is per-action: log, and either answer the packet the client is blocked on or send
//! nothing. Nothing here tears a session down, matching the vendor/loot/combat arms.

use super::super::*;
use wow_world_messages::vanilla::SMSG_SEND_MAIL_RESULT_MailResultTwo;

pub(crate) fn handle_mail<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<Option<ClientOpcodeMessage>> {
    match msg {
        // The 1.12 client opens the mail frame ITSELF on a type-19 gameobject (there is no
        // SMSG_SHOW_MAILBOX in vanilla) and then asks for the list. An empty mailbox still replies:
        // "no mail" and "the server ignored me" must not look the same. A refused gate sends
        // nothing — the same silent drop the out-of-range gameobject and unfriendly-vendor gates
        // take, since vanilla has no mailbox-refusal packet.
        ClientOpcodeMessage::CMSG_GET_MAIL_LIST(c) => {
            let self_guid = social::self_guid(conn);
            match mail::open_mailbox(store, self_guid, c.mailbox.guid()) {
                Ok(mails) => send(
                    tx,
                    Outbound::One(ServerOpcodeMessage::SMSG_MAIL_LIST_RESULT(Box::new(
                        codec::build_mail_list(&mails, now_secs()),
                    ))),
                )?,
                Err(e) => log::debug!(
                    "world: mail list refused (account {}): {e}",
                    conn.account_id
                ),
            }
        }
        // The periodic "do I have new mail" poll behind the minimap envelope. It names no mailbox —
        // the client sends it from anywhere — so the only gate is being in world. A refusal is
        // answered as "no unread mail" rather than with silence: the client repeats this poll and a
        // dropped reply leaves a stale envelope lit.
        ClientOpcodeMessage::MSG_QUERY_NEXT_MAIL_TIME => {
            let unread = mail::has_unread(store, social::self_guid(conn)).unwrap_or_else(|e| {
                log::debug!(
                    "world: mail poll answered empty (account {}): {e}",
                    conn.account_id
                );
                false
            });
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::MSG_QUERY_NEXT_MAIL_TIME(
                    codec::build_next_mail_time(unread),
                )),
            )?;
        }
        // The letter body. It does not ride the list packet: the list advertises the mail's own id
        // as an `item_text_id` and the client fetches the text here. A body it cannot have (another
        // player's mail, a deleted one) answers with EMPTY text rather than silence — the client has
        // already opened the letter and is waiting on this packet.
        ClientOpcodeMessage::CMSG_ITEM_TEXT_QUERY(c) => {
            let body = mail::letter_body(store, social::self_guid(conn), u64::from(c.mail_id))
                .unwrap_or_else(|e| {
                    log::debug!(
                        "world: item text query refused (account {}): {e}",
                        conn.account_id
                    );
                    None
                });
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_ITEM_TEXT_QUERY_RESPONSE(
                    Box::new(codec::build_item_text_response(
                        c.item_text_id,
                        body.unwrap_or_default(),
                    )),
                )),
            )?;
        }
        // Flip a mail's read state. Vanilla sends NO reply for this opcode — the client already
        // flipped its own display — so success and a refusal both answer with silence; only the
        // next CMSG_GET_MAIL_LIST shows the truth. A crafted id (someone else's mail, or a stale
        // one) is refused the same as a genuine miss — never trust a client-supplied mail id.
        ClientOpcodeMessage::CMSG_MAIL_MARK_AS_READ(c) => {
            let self_guid = social::self_guid(conn);
            if let Err(e) =
                mail::mark_read(store, self_guid, c.mailbox.guid(), u64::from(c.mail_id))
            {
                log::debug!(
                    "world: mail mark-as-read refused (account {}): {e}",
                    conn.account_id
                );
            }
        }
        // Delete a mail — destroys any attachment it still holds, as vanilla does (the confirmation
        // prompt is client-side). Unlike the read-only arms above, `CMSG_MAIL_DELETE` has a real
        // ack (`SMSG_SEND_MAIL_RESULT`/Deleted), so both outcomes reply through it, matching the
        // vendor/loot arms' "a failed action still answers" rule.
        ClientOpcodeMessage::CMSG_MAIL_DELETE(c) => {
            let self_guid = social::self_guid(conn);
            let ok = match mail::delete(store, self_guid, c.mailbox_id.guid(), u64::from(c.mail_id))
            {
                Ok(()) => true,
                Err(e) => {
                    log::debug!(
                        "world: mail delete refused (account {}): {e}",
                        conn.account_id
                    );
                    false
                }
            };
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_SEND_MAIL_RESULT(Box::new(
                    codec::build_mail_delete_result(c.mail_id, ok),
                ))),
            )?;
        }
        // Take a mail's copper into the purse. The mail id is client-supplied, so the refusal for
        // somebody else's mail is the authorization boundary and not a sanity check — and it reads
        // the same as "there is nothing in it", so a crafted id learns nothing either way. Both
        // outcomes ack through `SMSG_SEND_MAIL_RESULT`/MoneyTaken, which is what closes the
        // client's spinner.
        ClientOpcodeMessage::CMSG_MAIL_TAKE_MONEY(c) => {
            let self_guid = social::self_guid(conn);
            let ok =
                match mail::take_money(store, self_guid, c.mailbox.guid(), u64::from(c.mail_id)) {
                    Ok(()) => true,
                    Err(e) => {
                        log::debug!(
                            "world: mail take-money refused (account {}): {e}",
                            conn.account_id
                        );
                        false
                    }
                };
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_SEND_MAIL_RESULT(Box::new(
                    codec::build_mail_take_money_result(c.mail_id, ok),
                ))),
            )?;
        }
        // Take a mail's attached item into the bags. A full bag answers `ErrEquipError` and the
        // item STAYS in the letter — the case where a naive implementation destroys it — while
        // every other refusal reads the same, so a crafted mail id learns nothing.
        ClientOpcodeMessage::CMSG_MAIL_TAKE_ITEM(c) => {
            let self_guid = social::self_guid(conn);
            let outcome =
                match mail::take_item(store, self_guid, c.mailbox.guid(), u64::from(c.mail_id)) {
                    Ok(taken) => Ok(taken),
                    Err(e) => {
                        log::debug!(
                            "world: mail take-item refused (account {}): {e}",
                            conn.account_id
                        );
                        Err(match e {
                            mail::TakeItemRefusal::BagsFull(_) => {
                                codec::MailTakeItemError::BagsFull
                            }
                            mail::TakeItemRefusal::Other(_) => codec::MailTakeItemError::Other,
                        })
                    }
                };
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_SEND_MAIL_RESULT(Box::new(
                    codec::build_mail_take_item_result(c.mail_id, outcome),
                ))),
            )?;
        }
        // Post a letter. `cash_on_delivery_amount` rides this packet too and is deliberately
        // ignored here — COD is a later slice. Every refusal answers with its OWN
        // `MailResultTwo`, never a generic internal error: the client renders each as distinct
        // on-screen text, which is all the player gets to work with.
        //
        // The one refusal that answers with SILENCE is the mailbox gate, matching the list arm
        // above — vanilla has no mailbox-refusal packet, and the client only offers Send at a
        // mailbox, so reaching it means a crafted packet or a desynced session.
        ClientOpcodeMessage::CMSG_SEND_MAIL(c) => {
            let self_guid = social::self_guid(conn);
            let result2 = match mail::send(
                store,
                self_guid,
                c.mailbox.guid(),
                &c.receiver,
                c.subject.clone(),
                c.body.clone(),
                c.money.as_int(),
                c.item.guid(),
            ) {
                Ok(()) => Some(SMSG_SEND_MAIL_RESULT_MailResultTwo::Ok),
                Err(e) => {
                    log::debug!(
                        "world: mail send refused (account {}): {e}",
                        conn.account_id
                    );
                    match e {
                        mail::SendRefusal::NoMailbox(_) => None,
                        mail::SendRefusal::RecipientNotFound(_) => {
                            Some(SMSG_SEND_MAIL_RESULT_MailResultTwo::ErrRecipientNotFound)
                        }
                        mail::SendRefusal::CannotSendToSelf => {
                            Some(SMSG_SEND_MAIL_RESULT_MailResultTwo::ErrCannotSendToSelf)
                        }
                        mail::SendRefusal::NotYourTeam => {
                            Some(SMSG_SEND_MAIL_RESULT_MailResultTwo::ErrNotYourTeam)
                        }
                        mail::SendRefusal::NotEnoughMoney(_) => {
                            Some(SMSG_SEND_MAIL_RESULT_MailResultTwo::ErrNotEnoughMoney)
                        }
                        // A bound item and one that is not the sender's are different mistakes, so
                        // the client is told which: vanilla has no "soulbound" mail line, and
                        // `ErrCantSendWrappedCod` is mangoszero's nearest — it renders as a refusal
                        // about the attachment rather than about the letter.
                        mail::SendRefusal::AttachmentSoulbound(_) => {
                            Some(SMSG_SEND_MAIL_RESULT_MailResultTwo::ErrCantSendWrappedCod)
                        }
                        mail::SendRefusal::AttachmentInvalid(_) => {
                            Some(SMSG_SEND_MAIL_RESULT_MailResultTwo::ErrMailAttachmentInvalid)
                        }
                        mail::SendRefusal::Internal(_) => {
                            Some(SMSG_SEND_MAIL_RESULT_MailResultTwo::ErrInternalError)
                        }
                    }
                }
            };
            if let Some(result2) = result2 {
                send(
                    tx,
                    Outbound::One(ServerOpcodeMessage::SMSG_SEND_MAIL_RESULT(Box::new(
                        codec::build_mail_send_result(result2),
                    ))),
                )?;
            }
        }
        other => return Ok(Some(other)),
    }
    Ok(None)
}

/// Wall-clock seconds, the base of the expiry countdown the list packet stamps. Read here rather
/// than in the codec so the packet builder stays a pure function of its inputs.
fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
