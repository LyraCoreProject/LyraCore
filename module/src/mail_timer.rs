use lyracore_shared::mail::{MailSender, CHECK_MASK_COD_PAYMENT, CHECK_MASK_RETURNED};
use spacetimedb::{reducer, table, ReducerContext, ScheduleAt, Table, Timestamp};

use crate::mail::{game_mail, Mail};

const MICROS_PER_SEC: i64 = 1_000_000;

#[table(accessor = game_mail_timer, scheduled(fire_mail_timer))]
pub struct MailTimer {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    #[unique]
    pub mail_id: u64,
}

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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Expiry {
    Return,
    Delete,
}

pub(crate) fn expiry_of(mail: &Mail) -> Expiry {
    let from_character = matches!(mail.sender(), MailSender::Character(guid) if guid != 0);
    let sent_back_before = mail.check_flags & (CHECK_MASK_RETURNED | CHECK_MASK_COD_PAYMENT) != 0;
    if from_character && !sent_back_before && !mail.snapshot().is_empty() {
        Expiry::Return
    } else {
        Expiry::Delete
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Firing {
    Rearm(Timestamp),
    Deliver { expires: Timestamp },
    Expire(Expiry),
}

// Only a firing armed at the delivery instant may emit the delivery notification.
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

fn next_due(mail: &Mail, now: Timestamp) -> Timestamp {
    if mail.is_delivered(now) {
        expires_at(mail)
    } else {
        delivery(mail)
    }
}

pub(crate) fn start(ctx: &ReducerContext, mail: &Mail) {
    if mail.is_delivered(crate::time::now(ctx)) {
        arrive(ctx, mail.recipient_guid);
    }
    arm_next(ctx, mail);
}

pub(crate) fn arm_next(ctx: &ReducerContext, mail: &Mail) {
    arm(ctx, mail.id, next_due(mail, crate::time::now(ctx)));
}

pub(crate) fn stop(ctx: &ReducerContext, mail_id: u64) {
    ctx.db.game_mail_timer().mail_id().delete(mail_id);
}

// Fired rows remain until reducer return; delete before inserting the next unique mail_id.
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
        created_at: crate::time::now(ctx),
    });
    crate::native::mail_arrived(ctx, recipient_guid);
}

// Stagger overdue timers so a repair cannot unleash the whole backlog in consecutive transactions.
const BACKLOG_SPACING_MICROS: i64 = 10_000;

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
        .map(|mail| next_due(mail, crate::time::now(ctx)))
        .collect();
    for (mail, at) in untimed.iter().zip(spaced(&dues, crate::time::now(ctx))) {
        arm(ctx, mail.id, at);
    }
    untimed.len() as u64
}

#[reducer]
pub fn fire_mail_timer(ctx: &ReducerContext, timer: MailTimer) -> Result<(), String> {
    if ctx.sender() != ctx.database_identity() {
        return Err("scheduler only".to_string());
    }
    fire(ctx, &timer);
    Ok(())
}

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
        ScheduleAt::Interval(_) => crate::time::now(ctx),
    };
    match firing(&mail, due, crate::time::now(ctx)) {
        Firing::Rearm(at) => arm(ctx, mail.id, at),
        Firing::Deliver { expires } => {
            arrive(ctx, mail.recipient_guid);
            arm(ctx, mail.id, expires);
        }
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
