//! Letter Copy (`CMSG_MAIL_CREATE_TEXT_ITEM`): the durable text behind a mail's Plain Letter, and
//! the two reducers that create one. The mail and its text stay on the mail plane (Realm-core when
//! sharded, the single shard's own database otherwise); the Plain Letter itself is granted on the
//! Home Shard. The item sells for 0, so this is not a value flow: it needs neither Escrow nor a
//! Hold. Realm-core records the copy first, the Home Shard grants second, and a grant lost to a
//! race with the Gateway's own bag-room check costs nothing (README Decision 14).

use spacetimedb::{reducer, table, ReducerContext, Table};

use crate::mail::{game_mail, Mail};

/// cmangos `MAIL_BODY_ITEM_TEMPLATE` (`Mails/Mail.h:47`) — the "Plain Letter" a copied mail
/// becomes.
const MAIL_BODY_ITEM_TEMPLATE: u32 = 8383;

const NOTHING_TO_COPY: &str = "mail: this mail has no text to copy";
const ALREADY_COPIED: &str = "mail: this letter has already been made permanent";

/// A copied letter's text, keyed by the mail id narrowed to u32
/// (`lyracore_shared::mail::item_text_id_for`). Private: the Gateway reads it through the owner
/// token. Outlives the mail row that created it — vanilla lets a made-permanent letter keep its
/// text after the mail itself is gone, so this is a fact recorded once, not an escrow.
#[table(accessor = game_item_text)]
pub struct ItemText {
    #[primary_key]
    pub id: u32,
    pub text: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CopyTextPlan {
    NotYours,
    NoText,
    AlreadyCopied,
    Copy,
}

/// `row` is the delivered mail at `mail_id`, already scoped to whether it exists at all. Refuses a
/// mail that is not the caller's the same way a missing one does (a mail id is client-supplied), an
/// empty body the same way vmangos refuses an already-COPIED mail — the two read identically to a
/// crafted click, since `Letter::from_character` already marks an empty-body letter COPIED at
/// creation and a legacy row predates that convention.
pub(crate) fn plan_copy_text(row: Option<&Mail>, caller_guid: u64) -> CopyTextPlan {
    let Some(row) = row.filter(|m| m.recipient_guid == caller_guid) else {
        return CopyTextPlan::NotYours;
    };
    if row.body.is_empty() {
        return CopyTextPlan::NoText;
    }
    if row.check_flags & lyracore_shared::mail::CHECK_MASK_COPIED != 0 {
        return CopyTextPlan::AlreadyCopied;
    }
    CopyTextPlan::Copy
}

/// Sets COPIED on the mail and files its body as durable item text, keyed by the mail's own id.
/// This is the ONE-TRANSACTION half of a Letter Copy: `mail_text.rs` writes `game_mail` directly
/// (rather than through a `mail.rs` helper) because `module/src/mail.rs` is shared ground with the
/// Mail Timer and Auction Mail tickets landing in the same wave — a new function here keeps this
/// change out of their way.
pub(crate) fn apply_copy_text(
    ctx: &ReducerContext,
    recipient_guid: u64,
    mail_id: u64,
) -> Result<(), String> {
    let row = crate::mail::delivered_mail(ctx, mail_id);
    match plan_copy_text(row.as_ref(), recipient_guid) {
        CopyTextPlan::NotYours => return Err(lyracore_shared::mail::NOT_YOUR_MAIL.to_string()),
        CopyTextPlan::NoText => return Err(NOTHING_TO_COPY.to_string()),
        CopyTextPlan::AlreadyCopied => return Err(ALREADY_COPIED.to_string()),
        CopyTextPlan::Copy => {}
    }
    let row = row.expect("Copy is only reachable with a row");
    let text_id = lyracore_shared::mail::item_text_id_for(row.id, &row.body);
    ctx.db.game_item_text().insert(ItemText {
        id: text_id,
        text: row.body.clone(),
    });
    ctx.db.game_mail().id().update(Mail {
        check_flags: row.check_flags | lyracore_shared::mail::CHECK_MASK_COPIED,
        ..row
    });
    Ok(())
}

#[reducer]
pub fn realm_mail_copy_text(
    ctx: &ReducerContext,
    request_actor: crate::SessionActor,
    mail_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let recipient_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    apply_copy_text(ctx, recipient_guid, mail_id)
}

/// The Home Shard half: grant one Plain Letter carrying `item_text_id`. Refuses on full bags with
/// the existing `INVENTORY_FULL` text, which the Gateway's `mail_item_room` pre-check already tries
/// to avoid — this Gate is the real one, since bags can still fill between that check and this call.
#[reducer]
pub fn gw_mail_grant_letter(
    ctx: &ReducerContext,
    request_actor: crate::SessionActor,
    item_text_id: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let payee_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    crate::items::grant_letter_item(ctx, payee_guid, MAIL_BODY_ITEM_TEMPLATE, item_text_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyracore_shared::mail::MailSender;
    use spacetimedb::Timestamp;

    fn row(recipient_guid: u64, body: &str, check_flags: u32) -> Mail {
        let (sender_kind, sender_guid, sender_entry) = MailSender::Character(9).columns();
        Mail {
            id: 1,
            recipient_guid,
            sender_guid,
            subject: "Your sword".into(),
            body: body.into(),
            item_entry: 0,
            item_stack_count: 0,
            item_durability: 0,
            item_enchant_id: 0,
            item_soulbound: false,
            money: 0,
            cod: 0,
            was_read: false,
            created_at: Timestamp::from_micros_since_unix_epoch(1_000_000),
            random_property_id: 0,
            sender_kind,
            sender_entry,
            check_flags,
            mail_template_id: 0,
            deliver_micros: 0,
        }
    }

    #[test]
    fn copying_a_letter_is_refused_for_a_caller_who_is_not_the_recipient() {
        let mail = row(7, "meet me at the gate", 0x10);
        assert_eq!(plan_copy_text(Some(&mail), 7), CopyTextPlan::Copy);
        assert_eq!(plan_copy_text(Some(&mail), 8), CopyTextPlan::NotYours);
        assert_eq!(plan_copy_text(None, 7), CopyTextPlan::NotYours);
    }

    #[test]
    fn a_mail_with_no_body_has_nothing_to_copy() {
        // A character letter with an empty body is already COPIED at creation
        // (`Letter::from_character`), so this also covers that path — but a legacy row predating
        // the convention (empty body, no flags) must still be refused on the body alone.
        assert_eq!(
            plan_copy_text(Some(&row(7, "", 0)), 7),
            CopyTextPlan::NoText
        );
        assert_eq!(
            plan_copy_text(Some(&row(7, "", 0x04)), 7),
            CopyTextPlan::NoText,
            "an empty body is refused before the already-copied check even looks at the flag"
        );
    }

    #[test]
    fn a_second_copy_of_the_same_letter_is_refused() {
        let already = row(7, "meet me at the gate", 0x04);
        assert_eq!(
            plan_copy_text(Some(&already), 7),
            CopyTextPlan::AlreadyCopied
        );
        let both = row(7, "meet me at the gate", 0x10 | 0x04);
        assert_eq!(
            plan_copy_text(Some(&both), 7),
            CopyTextPlan::AlreadyCopied,
            "COPIED refuses even alongside HAS_BODY, since the flags OR together"
        );
    }

    #[test]
    fn the_realm_mail_copy_text_reducer_is_operator_gated() {
        let body =
            crate::test_scan::code_of(include_str!("mail_text.rs"), "pub fn realm_mail_copy_text(");
        let normalized: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            normalized.starts_with("{ crate::helpers::require_operator(ctx)?;"),
            "`realm_mail_copy_text` no longer OPENS with the operator gate. Body was:\n{body}"
        );
    }

    #[test]
    fn the_gw_mail_grant_letter_reducer_is_operator_gated() {
        let body =
            crate::test_scan::code_of(include_str!("mail_text.rs"), "pub fn gw_mail_grant_letter(");
        let normalized: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            normalized.starts_with("{ crate::helpers::require_operator(ctx)?;"),
            "`gw_mail_grant_letter` no longer OPENS with the operator gate. Body was:\n{body}"
        );
    }
}
