//! Mail Timer fixtures for `module/tests/mail_expiry.rs` and `module/tests/mail_delivery.rs`.
//! Each stage reducer writes its letters
//! through `mail::insert_letter` and then backdates them, so the timers they arm fire at once.
//! The checks compare armed instants with the vanilla lifetimes written out below, not with the
//! Module's own expiry function.

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
/// The seeded Tester. The take and COD fixtures pay out into its live purse and bags.
const TAKER: u64 = 1;
const ITEM: ItemSnapshot = ItemSnapshot {
    entry: 509_0070,
    stack_count: 2,
    durability: 17,
    enchant_id: 0,
    soulbound: false,
    random_property_id: 0,
    item_text_id: 0,
};
/// A real template, so a payout can grant it.
const BLADE: ItemSnapshot = ItemSnapshot {
    entry: crate::seed::FIXTURE_BLADE,
    stack_count: 1,
    durability: 17,
    enchant_id: 0,
    soulbound: false,
    random_property_id: 0,
    item_text_id: 0,
};
const SECOND_MICROS: i64 = 1_000_000;
const MINUTE_MICROS: i64 = 60 * SECOND_MICROS;
const DAY_MICROS: i64 = 86_400 * SECOND_MICROS;
/// A Mail lives 30 days after it arrives, 3 with a COD price (cmangos `Mail.cpp:309-313`), and a
/// Mail returned at expiry lives 30 days from the return (cmangos `ObjectMgr.cpp:6214-6215`).
const LIFE_MICROS: i64 = 30 * DAY_MICROS;
const COD_LIFE_MICROS: i64 = 3 * DAY_MICROS;
/// The delayed letter arrives this long after staging.
const DELAY_MICROS: i64 = 2 * SECOND_MICROS;

const RETURNS: &str = "timer: returns";
const COD_RETURNS: &str = "timer: cod returns";
const RETURNED: &str = "timer: returned";
const AUCTION: &str = "timer: auction";
const COD_PAYMENT: &str = "timer: cod payment";
const MONEY_ONLY: &str = "timer: money only";
const FRESH: &str = "timer: fresh";
const COD_FRESH: &str = "timer: cod fresh";
const DELAYED: &str = "timer: delayed";
const LEGACY_EXPIRED: &str = "timer: legacy expired";
const LEGACY_LIVE: &str = "timer: legacy live";
const TAKE_MONEY: &str = "timer: take money";
const TAKE_ITEM: &str = "timer: take item";
const COD_RACE: &str = "timer: cod race";

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
    let row = ctx
        .db
        .game_mail()
        .id()
        .find(id)
        .expect("insert_letter wrote the row");
    age(ctx, row, staged.age_micros)
}

/// Make `mail` `age_micros` old and arm its timer again.
fn age(ctx: &ReducerContext, mail: Mail, age_micros: i64) -> Mail {
    let aged = ctx.db.game_mail().id().update(Mail {
        created_at: Timestamp::from_micros_since_unix_epoch(
            ctx.timestamp.to_micros_since_unix_epoch() - age_micros,
        ),
        ..mail
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

fn armed_micros(ctx: &ReducerContext, mail_id: u64) -> Option<i64> {
    match ctx
        .db
        .game_mail_timer()
        .mail_id()
        .find(mail_id)?
        .scheduled_at
    {
        ScheduleAt::Time(at) => Some(at.to_micros_since_unix_epoch()),
        ScheduleAt::Interval(_) => None,
    }
}

/// `mail`'s timer fires `life_micros` after `arrived_micros`, to the second.
fn check_armed(
    ctx: &ReducerContext,
    mail: &Mail,
    arrived_micros: i64,
    life_micros: i64,
) -> Result<(), String> {
    let expected = arrived_micros + life_micros;
    match armed_micros(ctx, mail.id) {
        Some(armed) if (armed - expected).abs() <= SECOND_MICROS => Ok(()),
        armed => Err(format!(
            "{} is armed at {armed:?}, not {life_micros} µs after {arrived_micros}",
            mail.subject
        )),
    }
}

/// `subject` came back to SENDER as a Returned Mail with its item, no price and 30 days from the
/// return.
fn check_returned(ctx: &ReducerContext, subject: &str) -> Result<(), String> {
    let back = find(ctx, SENDER, subject).ok_or_else(|| format!("{subject} did not return"))?;
    if back.sender() != MailSender::Character(RECIPIENT)
        || back.check_flags != CHECK_MASK_RETURNED
        || back.cod != 0
        || back.was_read
        || back.snapshot() != ITEM
        || !back.is_delivered(ctx.timestamp)
    {
        return Err(format!("{subject} returned in the wrong shape"));
    }
    check_armed(ctx, &back, back.deliver_micros, LIFE_MICROS)
}

/// Stage one letter for each Mail Expiry outcome, two letters inside their life, and one letter
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
        COD_FRESH,
        DELAYED,
    ];
    for guid in [SENDER, RECIPIENT, DELAYED_RECIPIENT] {
        clear(ctx, guid, &subjects);
    }
    let (before, delayed_before) = (arrivals(ctx, RECIPIENT), arrivals(ctx, DELAYED_RECIPIENT));

    let expired = LIFE_MICROS + MINUTE_MICROS;
    let visible = [
        Staged::from_sender(RETURNS, expired),
        Staged {
            cod: 250,
            ..Staged::from_sender(COD_RETURNS, COD_LIFE_MICROS + MINUTE_MICROS)
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
        Staged::from_sender(FRESH, LIFE_MICROS - DAY_MICROS),
        Staged {
            cod: 250,
            ..Staged::from_sender(COD_FRESH, COD_LIFE_MICROS - 60 * MINUTE_MICROS)
        },
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
    if armed_micros(ctx, delayed.id) != Some(deliver_micros) {
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
    let fresh = find(ctx, RECIPIENT, FRESH).ok_or("a 29-day-old letter expired early")?;
    check_armed(
        ctx,
        &fresh,
        fresh.created_at.to_micros_since_unix_epoch(),
        LIFE_MICROS,
    )?;
    let cod_fresh = find(ctx, RECIPIENT, COD_FRESH).ok_or("a priced letter expired early")?;
    check_armed(
        ctx,
        &cod_fresh,
        cod_fresh.created_at.to_micros_since_unix_epoch(),
        COD_LIFE_MICROS,
    )?;
    let delayed = find(ctx, DELAYED_RECIPIENT, DELAYED).ok_or("the delayed letter is gone")?;
    if !delayed.is_delivered(ctx.timestamp) {
        return Err("the delayed letter is not delivered yet".to_string());
    }
    check_armed(ctx, &delayed, delayed.deliver_micros, LIFE_MICROS)?;
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
        (LEGACY_EXPIRED, LIFE_MICROS + MINUTE_MICROS),
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
    if back.sender() != MailSender::Character(LEGACY_RECIPIENT) {
        return Err("the expired legacy letter returned in the wrong shape".to_string());
    }
    check_armed(ctx, &back, back.deliver_micros, LIFE_MICROS)?;
    let live = find(ctx, LEGACY_RECIPIENT, LEGACY_LIVE).ok_or("the live legacy letter is gone")?;
    check_armed(
        ctx,
        &live,
        live.created_at.to_micros_since_unix_epoch(),
        LIFE_MICROS,
    )
}

/// Stage three letters for the Tester: 77 copper, a blade, and a blade for a 250-copper COD price.
/// The tests fence a take or a payment first and age the letter past its life after.
#[reducer]
pub fn debug_stage_mail_take_fixture(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    clear(ctx, TAKER, &[TAKE_MONEY, TAKE_ITEM, COD_RACE]);
    clear(ctx, SENDER, &[COD_RACE]);
    for letter in [
        Staged {
            money: 77,
            item: ItemSnapshot::default(),
            ..Staged::from_sender(TAKE_MONEY, 0)
        },
        Staged {
            item: BLADE,
            ..Staged::from_sender(TAKE_ITEM, 0)
        },
        Staged {
            item: BLADE,
            cod: 250,
            ..Staged::from_sender(COD_RACE, 0)
        },
    ] {
        stage(
            ctx,
            Staged {
                recipient_guid: TAKER,
                ..letter
            },
            0,
        );
    }
    Ok(())
}

/// Make the fixture letter `subject` for `recipient_guid` `age_secs` old, so a timer past its life
/// fires at once.
#[reducer]
pub fn debug_age_mail_fixture(
    ctx: &ReducerContext,
    recipient_guid: u64,
    subject: String,
    age_secs: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let mail = find(ctx, recipient_guid, &subject)
        .filter(|mail| mail.subject.starts_with("timer: "))
        .ok_or_else(|| format!("no fixture letter {subject} for {recipient_guid}"))?;
    let age_micros = i64::try_from(age_secs)
        .ok()
        .and_then(|secs| secs.checked_mul(SECOND_MICROS))
        .ok_or("age out of range")?;
    age(ctx, mail, age_micros);
    Ok(())
}

/// Bring the delayed letter `subject` for `recipient_guid` forward to arrive 1 µs from now, so its
/// Mail Timer fires at once and sends the Mail Arrival. Only for a "delivery: " letter that
/// `module/tests/mail_delivery.rs` committed with a Delivery Delay, and that has not arrived.
#[reducer]
pub fn debug_deliver_mail_fixture(
    ctx: &ReducerContext,
    recipient_guid: u64,
    subject: String,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let mail = find(ctx, recipient_guid, &subject)
        .filter(|mail| mail.subject.starts_with("delivery: "))
        .filter(|mail| !mail.is_delivered(ctx.timestamp))
        .ok_or_else(|| format!("no delayed fixture letter {subject} for {recipient_guid}"))?;
    // A timer delivers only when it fires at exactly `deliver_micros`, so the instant must stay
    // ahead of this transaction.
    let arrives = ctx.db.game_mail().id().update(Mail {
        deliver_micros: ctx.timestamp.to_micros_since_unix_epoch() + 1,
        ..mail
    });
    crate::mail_timer::arm_next(ctx, &arrives);
    Ok(())
}

/// Refuses unless `recipient_guid` holds the fixture letter `subject` as `held` says.
#[reducer]
pub fn debug_verify_mail_fixture_held(
    ctx: &ReducerContext,
    recipient_guid: u64,
    subject: String,
    held: bool,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if find(ctx, recipient_guid, &subject).is_some() == held {
        Ok(())
    } else {
        Err(format!("{recipient_guid} holding {subject} is not {held}"))
    }
}
