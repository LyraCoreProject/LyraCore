//! Mail Timer fixtures for `module/tests/mail_expiry.rs`. Each stage reducer writes its letters
//! through `mail::insert_letter` and then backdates them, so the timers they arm fire at once.

use lyracore_shared::mail::{
    MailSender, CHECK_MASK_COD_PAYMENT, CHECK_MASK_COPIED, CHECK_MASK_RETURNED,
};
use spacetimedb::{reducer, ReducerContext, ScheduleAt, Table, Timestamp};

use crate::items::ItemSnapshot;
use crate::mail::{game_mail, Letter, Mail};
use crate::mail_timer::{game_mail_arrival, game_mail_timer, MailTimer};

const SENDER: u64 = 509_0070;
const RECIPIENT: u64 = 509_0071;
const DELAYED_RECIPIENT: u64 = 509_0072;
const LEGACY_RECIPIENT: u64 = 509_0073;
/// The seeded Tester. The take fixture pays out into its live purse.
const TAKER: u64 = 1;
const ITEM: ItemSnapshot = ItemSnapshot {
    entry: 509_0070,
    stack_count: 2,
    durability: 17,
    enchant_id: 0,
    soulbound: false,
    random_property_id: 0,
};
const DAY_MICROS: i64 = 86_400_000_000;
const MINUTE_MICROS: i64 = 60_000_000;
/// The delayed letter arrives this long after staging.
const DELAY_MICROS: i64 = 2_000_000;

const RETURNS: &str = "timer: returns";
const COD_RETURNS: &str = "timer: cod returns";
const RETURNED: &str = "timer: returned";
const AUCTION: &str = "timer: auction";
const COD_PAYMENT: &str = "timer: cod payment";
const MONEY_ONLY: &str = "timer: money only";
const FRESH: &str = "timer: fresh";
const DELAYED: &str = "timer: delayed";
const LEGACY_EXPIRED: &str = "timer: legacy expired";
const LEGACY_LIVE: &str = "timer: legacy live";
const TAKE: &str = "timer: take";

/// One fixture letter and how old it is when staged.
struct Staged {
    subject: &'static str,
    recipient_guid: u64,
    sender: MailSender,
    check_flags: u32,
    money: u32,
    cod: u32,
    item: ItemSnapshot,
    age_micros: i64,
}

impl Staged {
    fn from_sender(subject: &'static str, age_micros: i64) -> Self {
        Self {
            subject,
            recipient_guid: RECIPIENT,
            sender: MailSender::Character(SENDER),
            check_flags: CHECK_MASK_COPIED,
            money: 0,
            cod: 0,
            item: ITEM,
            age_micros,
        }
    }
}

/// Insert `staged` as a new letter, then move its creation back by its age and arm its timer
/// again. Returns the backdated row.
fn stage(ctx: &ReducerContext, staged: Staged, deliver_micros: i64) -> Mail {
    let id = crate::mail::insert_letter(
        ctx,
        Letter {
            recipient_guid: staged.recipient_guid,
            sender: staged.sender,
            subject: staged.subject.to_string(),
            body: String::new(),
            money: staged.money,
            cod: staged.cod,
            item: staged.item,
            mail_template_id: 0,
            check_flags: staged.check_flags,
            deliver_micros,
        },
    );
    let mails = ctx.db.game_mail();
    let row = mails.id().find(id).expect("insert_letter wrote the row");
    let created_micros = row.created_at.to_micros_since_unix_epoch() - staged.age_micros;
    let aged = mails.id().update(Mail {
        created_at: Timestamp::from_micros_since_unix_epoch(created_micros),
        ..row
    });
    crate::mail_timer::arm_next(ctx, &aged);
    aged
}

/// Delete every Mail `recipient_guid` holds under one of `subjects`, with its timer.
fn clear(ctx: &ReducerContext, recipient_guid: u64, subjects: &[&str]) {
    let stale: Vec<u64> = ctx
        .db
        .game_mail()
        .by_recipient()
        .filter(&recipient_guid)
        .filter(|mail| subjects.contains(&mail.subject.as_str()))
        .map(|mail| mail.id)
        .collect();
    for id in stale {
        crate::mail::delete_mail(ctx, id);
    }
}

fn find(ctx: &ReducerContext, recipient_guid: u64, subject: &str) -> Option<Mail> {
    ctx.db
        .game_mail()
        .by_recipient()
        .filter(&recipient_guid)
        .find(|mail| mail.subject == subject)
}

fn arrivals(ctx: &ReducerContext, recipient_guid: u64) -> usize {
    ctx.db
        .game_mail_arrival()
        .by_recipient()
        .filter(&recipient_guid)
        .count()
}

fn armed_at(ctx: &ReducerContext, mail_id: u64) -> Option<ScheduleAt> {
    ctx.db
        .game_mail_timer()
        .mail_id()
        .find(mail_id)
        .map(|timer| timer.scheduled_at)
}

/// `subject` came back to SENDER as a Returned Mail with its item, no price and a fresh 30 days.
fn check_returned(ctx: &ReducerContext, subject: &str) -> Result<(), String> {
    let back = find(ctx, SENDER, subject).ok_or_else(|| format!("{subject} did not return"))?;
    if back.sender() != MailSender::Character(RECIPIENT)
        || back.check_flags != CHECK_MASK_RETURNED
        || back.cod != 0
        || back.was_read
        || back.snapshot() != ITEM
    {
        return Err(format!("{subject} returned in the wrong shape"));
    }
    let expires = crate::mail_timer::expires_at(&back);
    if expires.to_micros_since_unix_epoch()
        < ctx.timestamp.to_micros_since_unix_epoch() + 29 * DAY_MICROS
        || armed_at(ctx, back.id) != Some(ScheduleAt::Time(expires))
    {
        return Err(format!("{subject} has no fresh 30-day timer"));
    }
    Ok(())
}

/// `mail` is visible and waits for the end of its life.
fn check_waits_for_expiry(ctx: &ReducerContext, mail: &Mail) -> Result<(), String> {
    if !mail.is_delivered(ctx.timestamp)
        || armed_at(ctx, mail.id) != Some(ScheduleAt::Time(crate::mail_timer::expires_at(mail)))
    {
        return Err(format!("{} is not waiting for its expiry", mail.subject));
    }
    Ok(())
}

/// Stage one letter for each Mail Expiry outcome, one letter that is not due yet, and one letter
/// delivered in about 2 s. Refuses unless every visible letter sent exactly one Mail Arrival and
/// the delayed one sent none.
#[reducer]
pub fn debug_stage_mail_expiry_fixture(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let subjects = [
        RETURNS,
        COD_RETURNS,
        RETURNED,
        AUCTION,
        COD_PAYMENT,
        MONEY_ONLY,
        FRESH,
        DELAYED,
    ];
    for guid in [SENDER, RECIPIENT, DELAYED_RECIPIENT] {
        clear(ctx, guid, &subjects);
    }
    let (before, delayed_before) = (arrivals(ctx, RECIPIENT), arrivals(ctx, DELAYED_RECIPIENT));

    let expired = 30 * DAY_MICROS + MINUTE_MICROS;
    let visible = [
        Staged::from_sender(RETURNS, expired),
        Staged {
            cod: 250,
            ..Staged::from_sender(COD_RETURNS, 3 * DAY_MICROS + MINUTE_MICROS)
        },
        Staged {
            check_flags: CHECK_MASK_RETURNED,
            ..Staged::from_sender(RETURNED, expired)
        },
        Staged {
            sender: MailSender::AuctionHouse(7),
            ..Staged::from_sender(AUCTION, expired)
        },
        Staged {
            check_flags: CHECK_MASK_COD_PAYMENT,
            money: 250,
            item: ItemSnapshot::default(),
            ..Staged::from_sender(COD_PAYMENT, expired)
        },
        Staged {
            money: 100,
            item: ItemSnapshot::default(),
            ..Staged::from_sender(MONEY_ONLY, expired)
        },
        Staged::from_sender(FRESH, 29 * DAY_MICROS),
    ];
    let staged = visible.len();
    for letter in visible {
        stage(ctx, letter, 0);
    }
    let deliver_micros = ctx.timestamp.to_micros_since_unix_epoch() + DELAY_MICROS;
    let delayed = stage(
        ctx,
        Staged {
            recipient_guid: DELAYED_RECIPIENT,
            ..Staged::from_sender(DELAYED, 0)
        },
        deliver_micros,
    );

    if arrivals(ctx, RECIPIENT) - before != staged {
        return Err("each visible letter must send exactly one Mail Arrival".to_string());
    }
    if arrivals(ctx, DELAYED_RECIPIENT) != delayed_before {
        return Err("a letter delivered later sent a Mail Arrival early".to_string());
    }
    if armed_at(ctx, delayed.id)
        != Some(ScheduleAt::Time(Timestamp::from_micros_since_unix_epoch(
            deliver_micros,
        )))
    {
        return Err("the delayed letter's timer is not armed at its delivery".to_string());
    }
    Ok(())
}

/// Refuses until every staged letter has met its Mail Expiry outcome and the delayed letter has
/// been delivered.
#[reducer]
pub fn debug_verify_mail_expiry_fixture(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    check_returned(ctx, RETURNS)?;
    check_returned(ctx, COD_RETURNS)?;
    for subject in [
        RETURNS,
        COD_RETURNS,
        RETURNED,
        AUCTION,
        COD_PAYMENT,
        MONEY_ONLY,
    ] {
        if find(ctx, RECIPIENT, subject).is_some() {
            return Err(format!("{subject} is still with its recipient"));
        }
    }
    for subject in [RETURNED, AUCTION, COD_PAYMENT, MONEY_ONLY] {
        if find(ctx, SENDER, subject).is_some() {
            return Err(format!("{subject} went back instead of being deleted"));
        }
    }
    let fresh = find(ctx, RECIPIENT, FRESH).ok_or("a letter expired early")?;
    check_waits_for_expiry(ctx, &fresh)?;
    let delayed = find(ctx, DELAYED_RECIPIENT, DELAYED).ok_or("the delayed letter is gone")?;
    check_waits_for_expiry(ctx, &delayed)?;
    let mails = ctx.db.game_mail();
    if ctx
        .db
        .game_mail_timer()
        .iter()
        .any(|timer| mails.id().find(timer.mail_id).is_none())
    {
        return Err("a deleted Mail kept its timer".to_string());
    }
    Ok(())
}

/// Run the timer body again for every Mail id the expiry fixture used, with a row that is no
/// longer armed. Refuses if that changes a Mail, a timer or the Mail Arrivals.
#[reducer]
pub fn debug_replay_mail_timer_fixture(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    // The stage inserts RETURNS first and DELAYED last in one transaction, so the ids between
    // them are exactly the fixture's, deleted ones included.
    let first = find(ctx, SENDER, RETURNS).ok_or("stage and settle the fixture first")?;
    let last = find(ctx, DELAYED_RECIPIENT, DELAYED).ok_or("stage the fixture first")?;
    let state = |ctx: &ReducerContext, id: u64| -> Result<Vec<u8>, String> {
        let mut bytes = spacetimedb::sats::bsatn::to_vec(&ctx.db.game_mail().id().find(id))
            .map_err(|e| e.to_string())?;
        bytes.extend(
            spacetimedb::sats::bsatn::to_vec(&ctx.db.game_mail_timer().mail_id().find(id))
                .map_err(|e| e.to_string())?,
        );
        Ok(bytes)
    };
    let arrivals_before = ctx.db.game_mail_arrival().count();
    for mail_id in first.id..=last.id {
        let before = state(ctx, mail_id)?;
        crate::mail_timer::fire(
            ctx,
            &MailTimer {
                scheduled_id: 0,
                scheduled_at: ScheduleAt::Time(ctx.timestamp),
                mail_id,
            },
        );
        if state(ctx, mail_id)? != before {
            return Err(format!("replaying mail {mail_id}'s timer changed it"));
        }
    }
    if ctx.db.game_mail_arrival().count() != arrivals_before {
        return Err("a replayed timer sent a Mail Arrival".to_string());
    }
    Ok(())
}

/// Stage two letters as they were before the Mail Timer existed: one past its life and one inside
/// it, neither with a timer.
#[reducer]
pub fn debug_stage_mail_legacy_fixture(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    for guid in [SENDER, LEGACY_RECIPIENT] {
        clear(ctx, guid, &[LEGACY_EXPIRED, LEGACY_LIVE]);
    }
    for (subject, age_micros) in [
        (LEGACY_EXPIRED, 30 * DAY_MICROS + MINUTE_MICROS),
        (LEGACY_LIVE, DAY_MICROS),
    ] {
        let legacy = stage(
            ctx,
            Staged {
                recipient_guid: LEGACY_RECIPIENT,
                ..Staged::from_sender(subject, age_micros)
            },
            0,
        );
        crate::mail_timer::stop(ctx, legacy.id);
    }
    Ok(())
}

/// Refuses until the repair pass has armed both legacy letters and the expired one has returned.
#[reducer]
pub fn debug_verify_mail_legacy_fixture(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let back =
        find(ctx, SENDER, LEGACY_EXPIRED).ok_or("the expired legacy letter did not return")?;
    if back.sender() != MailSender::Character(LEGACY_RECIPIENT)
        || armed_at(ctx, back.id) != Some(ScheduleAt::Time(crate::mail_timer::expires_at(&back)))
    {
        return Err("the expired legacy letter returned in the wrong shape".to_string());
    }
    let live = find(ctx, LEGACY_RECIPIENT, LEGACY_LIVE).ok_or("the live legacy letter is gone")?;
    check_waits_for_expiry(ctx, &live)
}

/// Stage a 77-copper letter for the Tester. The test fences its copper, ages it past its life, and
/// pays the fence out after Mail Expiry deletes the row.
#[reducer]
pub fn debug_stage_mail_take_fixture(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    clear(ctx, TAKER, &[TAKE]);
    stage(
        ctx,
        Staged {
            recipient_guid: TAKER,
            money: 77,
            item: ItemSnapshot::default(),
            ..Staged::from_sender(TAKE, 0)
        },
        0,
    );
    Ok(())
}

/// Move the take letter past the end of its life, so its timer fires at once.
#[reducer]
pub fn debug_age_mail_take_fixture(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let mail = find(ctx, TAKER, TAKE).ok_or("stage the take fixture first")?;
    let aged = ctx.db.game_mail().id().update(Mail {
        created_at: Timestamp::from_micros_since_unix_epoch(
            mail.created_at.to_micros_since_unix_epoch() - 30 * DAY_MICROS - MINUTE_MICROS,
        ),
        ..mail
    });
    crate::mail_timer::arm_next(ctx, &aged);
    Ok(())
}

/// Refuses while the take letter still exists.
#[reducer]
pub fn debug_verify_mail_take_fixture(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    match find(ctx, TAKER, TAKE) {
        Some(_) => Err("the take letter has not expired".to_string()),
        None => Ok(()),
    }
}
