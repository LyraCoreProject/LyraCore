//! The Mail Timer: one one-shot schedule per Mail. It fires at the delivery instant of a Mail that
//! is not delivered yet, and at the end of every Mail's life to run Mail Expiry. Mail rows live on
//! Realm-core in a sharded realm, so the timers and the Mail Arrival events live there too.

use lyracore_shared::mail::{MailSender, CHECK_MASK_COD_PAYMENT, CHECK_MASK_RETURNED};
use spacetimedb::{reducer, table, ReducerContext, ScheduleAt, Table, Timestamp};

use crate::mail::{game_mail, Mail};

const MICROS_PER_SEC: i64 = 1_000_000;

/// The next firing of one Mail. `mail_id` is unique, so a Mail has at most one.
#[table(accessor = game_mail_timer, scheduled(fire_mail_timer))]
pub struct MailTimer {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    #[unique]
    pub mail_id: u64,
}

/// A Mail became visible to `recipient_guid`. The Gateway relays it as `SMSG_RECEIVED_MAIL` to that
/// recipient's World Session on any Shard, and the event GC reaps it.
#[table(
    accessor = game_mail_arrival,
    index(accessor = by_recipient, btree(columns = [recipient_guid]))
)]
pub struct MailArrival {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub recipient_guid: u64,
    pub created_at: Timestamp,
}

/// The end of a Mail's life: 3 days after it arrives with a cash on delivery price, else 30 days
/// (cmangos `Mail.cpp:301-313`). Whole seconds, rounded as the Gateway rounds the list countdown,
/// so the countdown reaches zero when the Mail expires.
pub(crate) fn expires_at(mail: &Mail) -> Timestamp {
    let secs = lyracore_shared::mail::expires_at_secs(
        mail.created_at
            .to_micros_since_unix_epoch()
            .div_euclid(MICROS_PER_SEC),
        mail.deliver_micros
            .saturating_add(MICROS_PER_SEC - 1)
            .div_euclid(MICROS_PER_SEC),
        mail.cod,
    );
    Timestamp::from_micros_since_unix_epoch(secs.saturating_mul(MICROS_PER_SEC))
}

/// What Mail Expiry does with a Mail (cmangos `ObjectMgr.cpp:6188-6232`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Expiry {
    /// Back to its Character sender in place, as a Returned Mail.
    Return,
    /// Deleted with its item and copper.
    Delete,
}

/// Only a Character's Mail that still carries an item goes back. A Returned Mail, a COD payment,
/// any other Mail Sender and a Mail from guid 0, which has nobody to take it back, are deleted.
/// So is a Mail without an item: its copper goes with it, as in cmangos and vmangos.
pub(crate) fn expiry_of(mail: &Mail) -> Expiry {
    let from_character = matches!(mail.sender(), MailSender::Character(guid) if guid != 0);
    let sent_back_before = mail.check_flags & (CHECK_MASK_RETURNED | CHECK_MASK_COD_PAYMENT) != 0;
    if from_character && !sent_back_before && !mail.snapshot().is_empty() {
        Expiry::Return
    } else {
        Expiry::Delete
    }
}

/// What one firing does with its Mail.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Firing {
    /// Too early: the Mail is not delivered yet, or its life grew after the timer was armed
    /// because its cash on delivery price was paid. Fire again at this instant.
    Rearm(Timestamp),
    /// The Mail's delivery instant: send its Mail Arrival, then fire again at its expiry.
    Deliver {
        expires: Timestamp,
    },
    Expire(Expiry),
}

/// `due` is the instant the firing was armed for. The scheduler never runs it earlier, so
/// `now >= due`. Only a firing armed at the delivery instant delivers.
pub(crate) fn firing(mail: &Mail, due: Timestamp, now: Timestamp) -> Firing {
    if !mail.is_delivered(now) {
        return Firing::Rearm(delivery(mail));
    }
    let expires = expires_at(mail);
    if now >= expires {
        Firing::Expire(expiry_of(mail))
    } else if due.to_micros_since_unix_epoch() == mail.deliver_micros {
        Firing::Deliver { expires }
    } else {
        Firing::Rearm(expires)
    }
}

fn delivery(mail: &Mail) -> Timestamp {
    Timestamp::from_micros_since_unix_epoch(mail.deliver_micros)
}

/// The instant a Mail's timer fires next: its delivery while it is not delivered, else its expiry.
fn next_due(mail: &Mail, now: Timestamp) -> Timestamp {
    if mail.is_delivered(now) {
        expires_at(mail)
    } else {
        delivery(mail)
    }
}

/// Start `mail`'s life with its current recipient: send a Mail Arrival when that recipient can see
/// it now, and arm its timer. A new Mail and a Returned Mail both come here.
pub(crate) fn start(ctx: &ReducerContext, mail: &Mail) {
    if mail.is_delivered(ctx.timestamp) {
        arrive(ctx, mail.recipient_guid);
    }
    arm_next(ctx, mail);
}

/// Arm `mail`'s timer for its next firing, replacing any timer it has.
pub(crate) fn arm_next(ctx: &ReducerContext, mail: &Mail) {
    arm(ctx, mail.id, next_due(mail, ctx.timestamp));
}

/// Cancel a Mail's timer. Every path that deletes a Mail row calls this.
pub(crate) fn stop(ctx: &ReducerContext, mail_id: u64) {
    ctx.db.game_mail_timer().mail_id().delete(mail_id);
}

/// A fired row stays until its reducer returns, so delete by `mail_id` before the insert, or the
/// unique index refuses the next row.
fn arm(ctx: &ReducerContext, mail_id: u64, at: Timestamp) {
    let timers = ctx.db.game_mail_timer();
    timers.mail_id().delete(mail_id);
    timers.insert(MailTimer {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Time(at),
        mail_id,
    });
}

fn arrive(ctx: &ReducerContext, recipient_guid: u64) {
    ctx.db.game_mail_arrival().insert(MailArrival {
        id: 0,
        recipient_guid,
        created_at: ctx.timestamp,
    });
}

/// The gap between two past-due timers that one repair pass arms. Each firing is its own
/// transaction and may write one Mail Arrival. Arming a backlog at the same past instant would
/// run every firing back to back after the pass commits. At 10 ms the backlog fires at 100
/// transactions a second, and 100,000 legacy Mail drain in under 17 minutes. Staggering keeps the
/// work in the one pass the Operator already runs, where a cap would leave Mail untimed until the
/// next publish.
const BACKLOG_SPACING_MICROS: i64 = 10_000;

/// When the timers a repair pass arms fire, in the order of `dues`. A future instant stays. The
/// `n`th past-due instant, counted from 1, moves to `now + n × 10 ms`.
fn spaced(dues: &[Timestamp], now: Timestamp) -> Vec<Timestamp> {
    let mut past_due = 0;
    dues.iter()
        .map(|&due| {
            if due > now {
                return due;
            }
            past_due += 1;
            Timestamp::from_micros_since_unix_epoch(
                now.to_micros_since_unix_epoch() + past_due * BACKLOG_SPACING_MICROS,
            )
        })
        .collect()
}

/// Arm a timer for every Mail that has none: Mail written before the timer existed, or Mail a
/// Transfer imported. Mail past its life expires shortly after this transaction commits, spaced
/// by [`BACKLOG_SPACING_MICROS`]. Returns how many timers it armed. Its caller,
/// `debug_repair_after_publish`, is a debug reducer.
#[cfg_attr(not(feature = "debug_reducers"), allow(dead_code))]
pub(crate) fn arm_missing(ctx: &ReducerContext) -> u64 {
    let timers = ctx.db.game_mail_timer();
    let untimed: Vec<Mail> = ctx
        .db
        .game_mail()
        .iter()
        .filter(|mail| timers.mail_id().find(mail.id).is_none())
        .collect();
    let dues: Vec<Timestamp> = untimed
        .iter()
        .map(|mail| next_due(mail, ctx.timestamp))
        .collect();
    for (mail, at) in untimed.iter().zip(spaced(&dues, ctx.timestamp)) {
        arm(ctx, mail.id, at);
    }
    untimed.len() as u64
}

/// Scheduler-only.
#[reducer]
pub fn fire_mail_timer(ctx: &ReducerContext, timer: MailTimer) -> Result<(), String> {
    if ctx.sender() != ctx.database_identity() {
        return Err("scheduler only".to_string());
    }
    fire(ctx, &timer);
    Ok(())
}

/// One firing of `timer`. A firing that is no longer its Mail's armed timer, or whose Mail is gone,
/// changes nothing, so a replay is a no-op.
pub(crate) fn fire(ctx: &ReducerContext, timer: &MailTimer) {
    let armed = ctx.db.game_mail_timer().mail_id().find(timer.mail_id);
    if armed.is_some_and(|armed| armed.scheduled_id != timer.scheduled_id) {
        return;
    }
    let Some(mail) = ctx.db.game_mail().id().find(timer.mail_id) else {
        return;
    };
    let due = match timer.scheduled_at {
        ScheduleAt::Time(at) => at,
        // `arm` writes only instants, so no interval row exists. Read one as due now.
        ScheduleAt::Interval(_) => ctx.timestamp,
    };
    match firing(&mail, due, ctx.timestamp) {
        Firing::Rearm(at) => arm(ctx, mail.id, at),
        Firing::Deliver { expires } => {
            arrive(ctx, mail.recipient_guid);
            arm(ctx, mail.id, expires);
        }
        // Auto-return arrives at once (cmangos `ObjectMgr.cpp:6214-6215`).
        Firing::Expire(Expiry::Return) => crate::mail::send_back(ctx, mail, 0),
        Firing::Expire(Expiry::Delete) => crate::mail::delete_mail(ctx, mail.id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyracore_shared::mail::{CHECK_MASK_COPIED, CHECK_MASK_HAS_BODY};

    const DAY: i64 = 86_400 * MICROS_PER_SEC;
    const SENT: i64 = 1_000 * MICROS_PER_SEC;

    fn at(micros: i64) -> Timestamp {
        Timestamp::from_micros_since_unix_epoch(micros)
    }

    fn mail(sender: MailSender) -> Mail {
        let (sender_kind, sender_guid, sender_entry) = sender.columns();
        Mail {
            id: 1,
            recipient_guid: 7,
            sender_guid,
            subject: "Your sword".into(),
            body: String::new(),
            item_entry: 0,
            item_stack_count: 0,
            item_durability: 0,
            item_enchant_id: 0,
            item_soulbound: false,
            money: 0,
            cod: 0,
            was_read: false,
            created_at: at(SENT),
            random_property_id: 0,
            sender_kind,
            sender_entry,
            check_flags: CHECK_MASK_COPIED,
            mail_template_id: 0,
            deliver_micros: 0,
            item_text_id: 0,
        }
    }

    fn with_item(sender: MailSender) -> Mail {
        Mail {
            item_entry: 509_0070,
            item_stack_count: 1,
            ..mail(sender)
        }
    }

    #[test]
    fn a_characters_mail_with_an_item_goes_back_at_expiry() {
        assert_eq!(
            expiry_of(&with_item(MailSender::Character(9))),
            Expiry::Return
        );
        let with_body = Mail {
            check_flags: CHECK_MASK_HAS_BODY,
            money: 100,
            ..with_item(MailSender::Character(9))
        };
        assert_eq!(
            expiry_of(&with_body),
            Expiry::Return,
            "copper beside an item travels back with it"
        );
    }

    #[test]
    fn returned_mail_cod_payments_and_other_senders_are_deleted_even_with_an_item() {
        let returned = Mail {
            check_flags: CHECK_MASK_RETURNED,
            ..with_item(MailSender::Character(9))
        };
        let cod_payment = Mail {
            check_flags: CHECK_MASK_COD_PAYMENT,
            ..with_item(MailSender::Character(9))
        };
        for (mail, what) in [
            (returned, "a Returned Mail"),
            (cod_payment, "a COD payment"),
            (with_item(MailSender::AuctionHouse(7)), "an Auction mail"),
            (with_item(MailSender::Creature(11_811)), "a Creature mail"),
            (
                with_item(MailSender::Gameobject(176_582)),
                "a Gameobject mail",
            ),
            (with_item(MailSender::Character(0)), "a mail from guid 0"),
        ] {
            assert_eq!(
                expiry_of(&mail),
                Expiry::Delete,
                "{what} (cmangos ObjectMgr.cpp:6205-6210)"
            );
        }
    }

    #[test]
    fn money_only_and_text_only_mail_is_deleted_with_its_copper() {
        let money_only = Mail {
            money: 100,
            ..mail(MailSender::Character(9))
        };
        assert_eq!(expiry_of(&money_only), Expiry::Delete);
        let text_only = Mail {
            body: "meet me at the gate".into(),
            check_flags: CHECK_MASK_HAS_BODY,
            ..mail(MailSender::Character(9))
        };
        assert_eq!(
            expiry_of(&text_only),
            Expiry::Delete,
            "cmangos ObjectMgr.cpp:6188,6232 returns only a mail with items"
        );
    }

    #[test]
    fn a_mail_expires_thirty_days_after_it_arrives_and_three_with_a_price() {
        let plain = with_item(MailSender::Character(9));
        let priced = Mail {
            cod: 250,
            ..with_item(MailSender::Character(9))
        };
        assert_eq!(expires_at(&plain), at(SENT + 30 * DAY));
        assert_eq!(expires_at(&priced), at(SENT + 3 * DAY));
        let delayed = Mail {
            deliver_micros: SENT + DAY,
            ..plain
        };
        assert_eq!(
            expires_at(&delayed),
            at(SENT + DAY + 30 * DAY),
            "the clock starts at delivery"
        );
    }

    #[test]
    fn the_expiry_instant_rounds_as_the_list_countdown_does() {
        let mid_second = Mail {
            created_at: at(SENT + 400_000),
            deliver_micros: SENT + 400_000,
            ..mail(MailSender::Character(9))
        };
        assert_eq!(
            expires_at(&mid_second),
            at(SENT + MICROS_PER_SEC + 30 * DAY),
            "the Gateway rounds a delivery up to the next second"
        );
    }

    #[test]
    fn a_firing_before_the_end_of_life_waits_and_at_the_end_the_mail_expires() {
        let m = with_item(MailSender::Character(9));
        let end = SENT + 30 * DAY;
        assert_eq!(firing(&m, at(end), at(end)), Firing::Expire(Expiry::Return));
        assert_eq!(
            firing(&m, at(end), at(end + DAY)),
            Firing::Expire(Expiry::Return),
            "a late firing still expires the mail"
        );
        assert_eq!(
            firing(&m, at(SENT + 3 * DAY), at(SENT + 3 * DAY)),
            Firing::Rearm(at(end)),
            "a paid cash on delivery price gives the letter its 30 days"
        );
    }

    #[test]
    fn a_mail_delivered_later_announces_itself_at_delivery_and_not_before() {
        let deliver = SENT + 3_600 * MICROS_PER_SEC;
        let delayed = Mail {
            deliver_micros: deliver,
            ..with_item(MailSender::Character(9))
        };
        assert_eq!(
            firing(&delayed, at(deliver - 1), at(deliver - 1)),
            Firing::Rearm(at(deliver)),
        );
        assert_eq!(
            firing(&delayed, at(deliver), at(deliver)),
            Firing::Deliver {
                expires: at(deliver + 30 * DAY)
            }
        );
        assert_eq!(
            firing(&delayed, at(deliver), at(deliver + 5 * MICROS_PER_SEC)),
            Firing::Deliver {
                expires: at(deliver + 30 * DAY)
            },
            "a late delivery firing still delivers"
        );
    }

    #[test]
    fn an_undelivered_mail_never_expires_early() {
        let delayed = Mail {
            deliver_micros: SENT + 40 * DAY,
            ..with_item(MailSender::Character(9))
        };
        let now = at(SENT + 31 * DAY);
        assert_eq!(
            firing(&delayed, now, now),
            Firing::Rearm(at(SENT + 40 * DAY)),
            "a mail created 31 days ago that arrives on day 40 waits for its delivery"
        );
    }

    #[test]
    fn a_new_mail_arms_at_delivery_when_it_is_not_delivered_yet_else_at_expiry() {
        let now = at(SENT);
        assert_eq!(
            next_due(&with_item(MailSender::Character(9)), now),
            at(SENT + 30 * DAY)
        );
        let delayed = Mail {
            deliver_micros: SENT + 3_600 * MICROS_PER_SEC,
            ..with_item(MailSender::Character(9))
        };
        assert_eq!(next_due(&delayed, now), at(SENT + 3_600 * MICROS_PER_SEC));
    }

    #[test]
    fn a_repair_pass_spaces_past_due_timers_10_ms_apart_and_keeps_future_ones() {
        let now_micros = SENT + 40 * DAY;
        let now = at(now_micros);
        assert_eq!(
            spaced(
                &[at(SENT + 30 * DAY), at(SENT + 50 * DAY), now, at(SENT)],
                now
            ),
            [
                at(now_micros + 10_000),
                at(SENT + 50 * DAY),
                at(now_micros + 20_000),
                at(now_micros + 30_000),
            ]
        );
    }

    #[test]
    fn a_mail_returned_at_expiry_lives_thirty_days_from_the_return() {
        let priced = Mail {
            cod: 250,
            ..with_item(MailSender::Character(9))
        };
        let returned_at = at(SENT + 3 * DAY + 5 * MICROS_PER_SEC);
        let back = crate::mail::returned(priced, returned_at);
        assert_eq!(back.cod, 0);
        assert_eq!(
            expires_at(&back),
            at(SENT + 3 * DAY + 5 * MICROS_PER_SEC + 30 * DAY),
            "cmangos ObjectMgr.cpp:6214-6215: expire_time = now + 30 days"
        );
    }
}
