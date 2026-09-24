//! Letter Copy (`CMSG_MAIL_CREATE_TEXT_ITEM`): the durable text behind a mail's Plain Letter, and
//! the two reducers that create one. The mail and its text stay on the mail plane (Realm-core when
//! sharded, the single shard's own database otherwise); the Plain Letter itself is granted on the
//! Home Shard. The item sells for 0, so this is not a value flow: it needs neither Escrow nor a
//! Hold. Realm-core records the copy first, the Home Shard grants second, and a grant lost to a
//! race with the Gateway's own bag-room check costs nothing.

use spacetimedb::{reducer, table, ReducerContext, Table};

use crate::game_item_instance;
use crate::mail::{game_mail, Mail};

/// cmangos `MAIL_BODY_ITEM_TEMPLATE` (`Mails/Mail.h:47`) — the "Plain Letter" a copied mail
/// becomes.
const MAIL_BODY_ITEM_TEMPLATE: u32 = 8383;

const NOTHING_TO_COPY: &str = "mail: this mail has no text to copy";
/// `item_text_id_for` narrows a mail id to `u32` and answers `0` — its empty-body sentinel — for
/// one it cannot narrow. A mail id this large will not occur under today's GUID ranges, but the
/// Gate must still refuse outright rather than let a `0`-keyed `ItemText` masquerade as a real
/// copy: that would mark the mail COPIED and (via a later grant) GRANTED over an unreadable
/// letter, with no bit left to retry from.
const ID_TOO_LARGE_TO_COPY: &str = "mail: this letter's id is too large to become a Plain Letter";

/// A copied letter's text, keyed by the mail id narrowed to u32
/// (`lyracore_shared::mail::item_text_id_for`). Private: the Gateway reads it through the owner
/// token. Outlives the mail row that created it, because vanilla lets a made-permanent letter keep
/// its text after the mail itself is gone. It is deleted with the last Plain Letter that carries it.
#[table(accessor = game_item_text)]
pub struct ItemText {
    #[primary_key]
    pub id: u32,
    pub text: String,
    /// How many Plain Letters can still carry this text. Each copy adds one before the Home Shard
    /// grants, so the count is never below the letters that exist; a grant that never lands only
    /// keeps the row longer. Each destroyed letter takes one away, and the row goes at 0. A row
    /// filed before the count existed reads [`UNCOUNTED_LETTERS`].
    #[default(UNCOUNTED_LETTERS)]
    pub letters: u32,
}

/// `ItemText.letters` of a row whose letters nobody counted. It is never deleted, because a letter
/// that still carries it would lose its text.
pub(crate) const UNCOUNTED_LETTERS: u32 = u32::MAX;

/// The count after one more copy of the text.
fn letters_after_copy(letters: u32) -> u32 {
    letters.saturating_add(1)
}

/// The count after one letter is destroyed, or `None` when the row goes.
fn letters_after_release(letters: u32) -> Option<u32> {
    match letters {
        UNCOUNTED_LETTERS => Some(UNCOUNTED_LETTERS),
        0 | 1 => None,
        n => Some(n - 1),
    }
}

/// One Plain Letter carrying `item_text_id` is destroyed. Its text goes with the last such letter.
/// A database that does not hold the text row changes nothing: on a sharded realm a Home Shard
/// holds letters, but their text is on Realm-core.
pub(crate) fn drop_letter_text(ctx: &ReducerContext, item_text_id: u32) {
    if item_text_id == 0 {
        return;
    }
    let texts = ctx.db.game_item_text();
    let Some(row) = texts.id().find(item_text_id) else {
        return;
    };
    match letters_after_release(row.letters) {
        None => {
            texts.id().delete(item_text_id);
        }
        Some(letters) if letters != row.letters => {
            texts.id().update(ItemText { letters, ..row });
        }
        Some(_) => {}
    }
}

const ALREADY_GRANTED: &str = "mail: this letter was already made permanent";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CopyTextPlan {
    NotYours,
    NoText,
    IdTooLarge,
    AlreadyGranted,
    Copy,
}

/// `row` is the delivered mail at `mail_id`, already scoped to whether it exists at all. Refuses a
/// mail that is not the caller's the same way a missing one does (a mail id is client-supplied), an
/// empty body the same way vmangos refuses an already-COPIED mail — the two read identically to a
/// crafted click, since `Letter::from_character` already marks an empty-body letter COPIED at
/// creation and a legacy row predates that convention.
///
/// The Gate checks GRANTED, not COPIED: COPIED alone only tells the vanilla client to hide the
/// letter button, and `apply_copy_text` treats it as a replay so a redundant click or a retry after
/// an interrupted grant can still reach the Home Shard. GRANTED is the durable record that the
/// Plain Letter actually landed — it survives the player destroying, mailing away, or trading that
/// item, which is what stops a copy-grant-destroy loop from minting the letter over and over.
pub(crate) fn plan_copy_text(row: Option<&Mail>, caller_guid: u64) -> CopyTextPlan {
    let Some(row) = row.filter(|m| m.recipient_guid == caller_guid) else {
        return CopyTextPlan::NotYours;
    };
    if row.body.is_empty() {
        return CopyTextPlan::NoText;
    }
    // Structural, ahead of GRANTED: whether this id can ever become a text id does not depend on
    // whether a grant already landed.
    if u32::try_from(row.id).is_err() {
        return CopyTextPlan::IdTooLarge;
    }
    if row.check_flags & lyracore_shared::mail::CHECK_FLAG_LETTER_GRANTED != 0 {
        return CopyTextPlan::AlreadyGranted;
    }
    CopyTextPlan::Copy
}

/// Sets COPIED on the mail and files its body as durable item text, keyed by the mail's own id.
/// This is the ONE-TRANSACTION half of a Letter Copy: `mail_text.rs` writes `game_mail` directly
/// (rather than through a `mail.rs` helper) because `module/src/mail.rs` is shared ground with
/// other mail work landing in the same window — a new function here keeps this change out of its
/// way.
///
/// Replay-safe: a retry that reaches here again before GRANTED is ever set (bags filled, logout,
/// timeout, a crash between here and the Home Shard grant) re-runs harmlessly. The text row is
/// reused when it already exists: `mail::returned` keeps a returned mail's id, so a letter a second
/// Character copies after the first sender gets it back reuses the same text id, and the body never
/// changes for a given mail id. Every call counts one more letter, because the Gateway grants only
/// after this call succeeds; a replay that grants nothing keeps the text row longer and loses
/// nothing. Setting COPIED again is a no-op OR.
pub(crate) fn apply_copy_text(
    ctx: &ReducerContext,
    recipient_guid: u64,
    mail_id: u64,
) -> Result<(), String> {
    let row = crate::mail::delivered_mail(ctx, mail_id);
    match plan_copy_text(row.as_ref(), recipient_guid) {
        CopyTextPlan::NotYours => return Err(lyracore_shared::mail::NOT_YOUR_MAIL.to_string()),
        CopyTextPlan::NoText => return Err(NOTHING_TO_COPY.to_string()),
        CopyTextPlan::IdTooLarge => return Err(ID_TOO_LARGE_TO_COPY.to_string()),
        CopyTextPlan::AlreadyGranted => return Err(ALREADY_GRANTED.to_string()),
        CopyTextPlan::Copy => {}
    }
    let row = row.expect("Copy is only reachable with a row");
    let text_id = lyracore_shared::mail::item_text_id_for(row.id, &row.body);
    let texts = ctx.db.game_item_text();
    match texts.id().find(text_id) {
        Some(text) => {
            texts.id().update(ItemText {
                letters: letters_after_copy(text.letters),
                ..text
            });
        }
        None => {
            texts.insert(ItemText {
                id: text_id,
                text: row.body.clone(),
                letters: letters_after_copy(0),
            });
        }
    }
    ctx.db.game_mail().id().update(Mail {
        check_flags: row.check_flags | lyracore_shared::mail::CHECK_MASK_COPIED,
        ..row
    });
    Ok(())
}

/// A deleted Character's letters are destroyed: the ones in its bags and the ones attached to its
/// Mail. Only the Character deletion reducers call this. A Transfer runs the same delete sweeps on
/// its source, but there the letters travel, so their text must stay.
pub(crate) fn drop_character_letters(ctx: &ReducerContext, character_guid: u64) {
    let in_bags = ctx
        .db
        .game_item_instance()
        .by_owner_guid()
        .filter(&character_guid)
        .map(|item| item.item_text_id);
    let in_mail = ctx
        .db
        .game_mail()
        .by_recipient()
        .filter(&character_guid)
        .map(|mail| mail.item_text_id);
    let letters: Vec<u32> = in_bags.chain(in_mail).filter(|id| *id != 0).collect();
    for item_text_id in letters {
        drop_letter_text(ctx, item_text_id);
    }
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
/// `grant_letter_item`'s own "the owner already holds this text id" check is a second, narrower
/// guard: it only covers the window between this call landing and `realm_mail_mark_letter_granted`
/// recording GRANTED, not the whole lifetime of the item.
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

/// Sets GRANTED on the mail plane once the Gateway has confirmed `gw_mail_grant_letter` returned
/// `Ok`. The durable record a Letter Copy landed: unlike the granted item itself, this bit cannot
/// be destroyed, mailed away, or traded, so it is what `plan_copy_text` checks to refuse a second
/// grant for good — not whether the player still happens to hold the item.
pub(crate) fn apply_mark_letter_granted(
    ctx: &ReducerContext,
    recipient_guid: u64,
    mail_id: u64,
) -> Result<(), String> {
    let row = crate::mail::delivered_mail(ctx, mail_id)
        .filter(|m| m.recipient_guid == recipient_guid)
        .ok_or_else(|| lyracore_shared::mail::NOT_YOUR_MAIL.to_string())?;
    if row.check_flags & lyracore_shared::mail::CHECK_FLAG_LETTER_GRANTED == 0 {
        ctx.db.game_mail().id().update(Mail {
            check_flags: row.check_flags | lyracore_shared::mail::CHECK_FLAG_LETTER_GRANTED,
            ..row
        });
    }
    Ok(())
}

#[reducer]
pub fn realm_mail_mark_letter_granted(
    ctx: &ReducerContext,
    request_actor: crate::SessionActor,
    mail_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let recipient_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    apply_mark_letter_granted(ctx, recipient_guid, mail_id)
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
            item_text_id: 0,
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
            "an empty body is refused before the GRANTED check even looks at the flags"
        );
    }

    #[test]
    fn a_mail_id_too_large_for_a_text_id_is_refused_rather_than_copied_unreadable() {
        // `item_text_id_for` narrows a mail id to `u32` and answers `0` — its empty-body
        // sentinel — for one that does not fit. Without this Gate, `apply_copy_text` would file a
        // `0`-keyed `ItemText`, mark the mail COPIED, and leave the player with an unreadable
        // letter and no bit left to retry from.
        let mut too_large = row(7, "meet me at the gate", 0);
        too_large.id = u64::from(u32::MAX) + 1;
        assert_eq!(
            plan_copy_text(Some(&too_large), 7),
            CopyTextPlan::IdTooLarge
        );
    }

    #[test]
    fn a_copy_that_only_set_copied_replays_rather_than_refusing() {
        // COPIED alone means an earlier attempt reached the mail plane but the Gateway never
        // confirmed the Home Shard grant landed — the shape a crash or a lost reply leaves behind.
        // The Gate must let a retry through so it can reach the grant.
        let copied_only = row(7, "meet me at the gate", 0x04);
        assert_eq!(plan_copy_text(Some(&copied_only), 7), CopyTextPlan::Copy);
        let with_has_body = row(7, "meet me at the gate", 0x10 | 0x04);
        assert_eq!(plan_copy_text(Some(&with_has_body), 7), CopyTextPlan::Copy);
    }

    #[test]
    fn a_letter_already_granted_is_refused_to_copy_again() {
        let granted = row(7, "meet me at the gate", 0x20);
        assert_eq!(
            plan_copy_text(Some(&granted), 7),
            CopyTextPlan::AlreadyGranted
        );
        let granted_and_copied = row(7, "meet me at the gate", 0x04 | 0x20);
        assert_eq!(
            plan_copy_text(Some(&granted_and_copied), 7),
            CopyTextPlan::AlreadyGranted,
            "GRANTED refuses regardless of which other bits ride alongside it"
        );
    }

    #[test]
    fn a_text_row_goes_with_its_last_letter() {
        assert_eq!(letters_after_copy(0), 1);
        assert_eq!(letters_after_copy(1), 2);
        assert_eq!(letters_after_release(2), Some(1));
        assert_eq!(letters_after_release(1), None);
        assert_eq!(letters_after_release(0), None);
    }

    #[test]
    fn an_uncounted_text_row_is_never_deleted() {
        assert_eq!(letters_after_release(u32::MAX), Some(u32::MAX));
        assert_eq!(letters_after_copy(u32::MAX), u32::MAX);
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

    #[test]
    fn the_realm_mail_mark_letter_granted_reducer_is_operator_gated() {
        let body = crate::test_scan::code_of(
            include_str!("mail_text.rs"),
            "pub fn realm_mail_mark_letter_granted(",
        );
        let normalized: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            normalized.starts_with("{ crate::helpers::require_operator(ctx)?;"),
            "`realm_mail_mark_letter_granted` no longer OPENS with the operator gate. Body was:\n{body}"
        );
    }
}
