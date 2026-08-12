//! Mail family: open the mailbox window, answer the client's mail poll, and serve a letter's body.
//!
//! Read path only — sending, attachments, take, mark-read, delete and return are later
//! slices, and their opcodes fall through to the next handler until they land.
//!
//! Every failure is per-action: log, and either answer the packet the client is blocked on or send
//! nothing. Nothing here tears a session down, matching the vendor/loot/combat arms.

use super::super::*;

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
