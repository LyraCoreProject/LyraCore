use spacetimedb::{reducer, table, ReducerContext, Table};

use crate::game_item_instance;
use crate::mail::{game_mail, Mail};

const MAIL_BODY_ITEM_TEMPLATE: u32 = 8383;

const NOTHING_TO_COPY: &str = "mail: this mail has no text to copy";
const ID_TOO_LARGE_TO_COPY: &str = "mail: this letter's id is too large to become a Plain Letter";

#[table(accessor = game_item_text)]
pub struct ItemText {
    #[primary_key]
    pub id: u32,
    pub text: String,
    // Count before granting so missed grants retain text without undercounting letters.
    #[default(UNCOUNTED_LETTERS)]
    pub letters: u32,
}

// Unknown legacy counts are never deleted because a surviving letter may still need this text.
pub(crate) const UNCOUNTED_LETTERS: u32 = u32::MAX;

fn letters_after_copy(letters: u32) -> u32 {
    letters.saturating_add(1)
}

fn letters_after_release(letters: u32) -> Option<u32> {
    match letters {
        UNCOUNTED_LETTERS => Some(UNCOUNTED_LETTERS),
        0 | 1 => None,
        n => Some(n - 1),
    }
}

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

pub(crate) fn plan_copy_text(row: Option<&Mail>, caller_guid: u64) -> CopyTextPlan {
    let Some(row) = row.filter(|m| m.recipient_guid == caller_guid) else {
        return CopyTextPlan::NotYours;
    };
    if row.body.is_empty() {
        return CopyTextPlan::NoText;
    }
    if u32::try_from(row.id).is_err() {
        return CopyTextPlan::IdTooLarge;
    }
    if row.check_flags & lyracore_shared::mail::CHECK_FLAG_LETTER_GRANTED != 0 {
        return CopyTextPlan::AlreadyGranted;
    }
    CopyTextPlan::Copy
}

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

pub(crate) fn apply_copy_letter(
    ctx: &ReducerContext,
    recipient_guid: u64,
    mail_id: u64,
) -> Result<(), String> {
    apply_copy_text(ctx, recipient_guid, mail_id)?;
    crate::items::grant_letter_item(ctx, recipient_guid, MAIL_BODY_ITEM_TEMPLATE, mail_id as u32)?;
    apply_mark_letter_granted(ctx, recipient_guid, mail_id)
}

// Transfer source sweeps move letters rather than destroying them, so their text must remain.
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
    request_actor: crate::account_ownership::SessionActor,
    mail_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let recipient_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    apply_copy_text(ctx, recipient_guid, mail_id)
}

#[reducer]
pub fn gw_mail_grant_letter(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    item_text_id: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let payee_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    crate::items::grant_letter_item(ctx, payee_guid, MAIL_BODY_ITEM_TEMPLATE, item_text_id)
}

// GRANTED survives item destruction and trade; possession cannot prove a prior Letter Copy grant.
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
    request_actor: crate::account_ownership::SessionActor,
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
        let mut too_large = row(7, "meet me at the gate", 0);
        too_large.id = u64::from(u32::MAX) + 1;
        assert_eq!(
            plan_copy_text(Some(&too_large), 7),
            CopyTextPlan::IdTooLarge
        );
    }

    #[test]
    fn a_copy_that_only_set_copied_replays_rather_than_refusing() {
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
}
