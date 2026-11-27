// vmangos `GameEventMgr::SendEventMails`; read docs/schema.md#game-events first.

use std::collections::BTreeMap;

use lyracore_shared::mail::{MailSender, CHECK_MASK_RETURNED};
use lyracore_shared::race_class::{mask_admits, ZeroMask};
use spacetimedb::{log, table, ReducerContext, Table};

use super::GameEventState;
use crate::auth::{guid_high_water, in_guid_range, own_guid_range};
use crate::game_character;
use crate::mail::Letter;
use crate::quest::game_character_quest;
use crate::transfer::game_transfer_in;

/// One vmangos `game_event_mail` row: a letter to each Character of `race_mask` when its event
/// starts.
#[table(accessor = game_event_mail, index(accessor = by_event, btree(columns = [event_entry])))]
pub struct GameEventMail {
    #[primary_key]
    pub id: u64,
    pub event_entry: u32,
    pub race_mask: u32,
    // Only Characters rewarded this quest receive the letter; 0 asks for none.
    pub quest_entry: u32,
    pub mail_template_id: u32,
    pub sender_entry: u32,
}

/// An event this Shard has started and not yet stopped, as vmangos `game_event_status` holds it.
#[table(accessor = game_event_started)]
pub struct GameEventStarted {
    #[primary_key]
    pub event_entry: u32,
}

/// The letters of one event start's mail row still to send, with the row's own values so a later
/// import cannot change them. Each Character held with a guid from `next_guid` that the mail
/// addresses gets one, unless it was created after the start: `own_mark` and `last_guid` bound
/// the guids that existed then.
#[table(accessor = game_event_mailing)]
pub struct GameEventMailing {
    #[primary_key]
    #[auto_inc]
    pub mailing_id: u64,
    pub event_entry: u32,
    pub race_mask: u32,
    pub quest_entry: u32,
    pub mail_template_id: u32,
    pub sender_entry: u32,
    pub next_guid: u64,
    // The highest guid this Shard had minted, or held when it has no allocator mark.
    pub own_mark: u64,
    // The highest guid this Shard held, which bounds guids minted by other Shards' ranges.
    pub last_guid: u64,
}

crate::outside_table_catalog!(game_event_mail, game_event_started, game_event_mailing);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edge {
    Start,
    Stop,
}

/// The edge an event crosses from what this Shard last followed. A started event that no longer
/// runs, stopped or unscheduled, has stopped, as vmangos `GameEventMgr::EnableEvent` stops an event
/// it disables.
fn edge(state: GameEventState, started: bool) -> Option<Edge> {
    match (state, started) {
        (GameEventState::Active, false) => Some(Edge::Start),
        (GameEventState::Inactive | GameEventState::Unscheduled, true) => Some(Edge::Stop),
        _ => None,
    }
}

/// Whether a Character of `race` receives the mail; `rewarded` tells whether the Character's reward
/// of the mail's quest is recorded.
fn addressed_to(mail: &GameEventMailing, race: u8, rewarded: impl FnOnce() -> bool) -> bool {
    mask_admits(mail.race_mask, race, ZeroMask::AdmitsNone) && (mail.quest_entry == 0 || rewarded())
}

/// Whether a Character with `guid` existed when the mailing started. A guid in this Shard's own
/// range existed if it is within the mark; a guid from another range, which a Transfer brought,
/// if it is within the highest guid held; with no range only the mark counts.
fn existed_at_start(mailing: &GameEventMailing, range: Option<(u64, u64)>, guid: u64) -> bool {
    if range.is_none() || in_guid_range(range, guid) {
        guid <= mailing.own_mark
    } else {
        guid <= mailing.last_guid
    }
}

/// Queues the mail of each event that started since this Shard last followed it, as vmangos
/// `ApplyNewEvent` does; a stop lets the event's next start mail again but keeps what is queued.
pub(super) fn follow(ctx: &ReducerContext, states: &BTreeMap<u32, GameEventState>) {
    let started = ctx.db.game_event_started();
    for (&event_entry, &state) in states {
        let Some(edge) = edge(state, started.event_entry().find(event_entry).is_some()) else {
            continue;
        };
        match edge {
            Edge::Start => {
                started.insert(GameEventStarted { event_entry });
                enqueue(ctx, event_entry);
            }
            Edge::Stop => {
                started.event_entry().delete(event_entry);
            }
        }
    }
}

/// The Characters this Shard holds, lowest guid first, as (guid, race). One still arriving by
/// Transfer is left out: the Shard it leaves sends its letter and mail routing takes it to the new
/// home.
fn held_characters(ctx: &ReducerContext) -> Vec<(u64, u8)> {
    let mut held: Vec<(u64, u8)> = ctx
        .db
        .game_character()
        .iter()
        .filter(|character| {
            ctx.db
                .game_transfer_in()
                .by_character()
                .filter(character.guid)
                .next()
                .is_none()
        })
        .map(|character| (character.guid, character.race))
        .collect();
    held.sort_unstable();
    held
}

/// Queues one mailing for each mail row of the event, closed at the guids this Shard holds now, as
/// vmangos reads its recipients when the event starts.
fn enqueue(ctx: &ReducerContext, event_entry: u32) {
    let rows: Vec<GameEventMail> = ctx
        .db
        .game_event_mail()
        .by_event()
        .filter(event_entry)
        .collect();
    if rows.is_empty() {
        return;
    }
    let Some(&(last_guid, _)) = held_characters(ctx).last() else {
        return;
    };
    let own_mark = guid_high_water(ctx).unwrap_or(last_guid);
    let mailings = ctx.db.game_event_mailing();
    for mail in rows {
        mailings.insert(GameEventMailing {
            mailing_id: 0,
            event_entry,
            race_mask: mail.race_mask,
            quest_entry: mail.quest_entry,
            mail_template_id: mail.mail_template_id,
            sender_entry: mail.sender_entry,
            next_guid: 0,
            own_mark,
            last_guid,
        });
    }
}

/// Sends at most `budget` letters of the queued mailings, oldest first, each Character once; a
/// mailing that has reached its last Character leaves the queue.
pub(super) fn deliver(ctx: &ReducerContext, mut budget: usize) {
    let mailings = ctx.db.game_event_mailing();
    let mut queue: Vec<GameEventMailing> = mailings.iter().collect();
    if queue.is_empty() {
        return;
    }
    queue.sort_unstable_by_key(|mailing| mailing.mailing_id);
    let held = held_characters(ctx);
    for mut mailing in queue {
        if budget == 0 {
            break;
        }
        if send_next(ctx, &mut mailing, &held, &mut budget) {
            mailings.mailing_id().delete(mailing.mailing_id);
        } else {
            mailings.mailing_id().update(mailing);
        }
    }
}

/// Sends the mailing's letters from its cursor until `budget` runs out; whether the mailing is done.
/// A mailing whose template is gone is done, with nothing more sent.
fn send_next(
    ctx: &ReducerContext,
    mailing: &mut GameEventMailing,
    held: &[(u64, u8)],
    budget: &mut usize,
) -> bool {
    let (body, item) = match crate::mail_reward::template_letter(ctx, mailing.mail_template_id) {
        Ok(contents) => contents,
        Err(missing) => {
            log::error!(
                "game event {}: mail template {} not sent ({missing})",
                mailing.event_entry,
                mailing.mail_template_id
            );
            return true;
        }
    };
    let range = own_guid_range(ctx);
    let from = held.partition_point(|&(guid, _)| guid < mailing.next_guid);
    for &(guid, race) in &held[from..] {
        if !existed_at_start(mailing, range, guid)
            || !addressed_to(mailing, race, || rewarded(ctx, guid, mailing.quest_entry))
        {
            continue;
        }
        if *budget == 0 {
            mailing.next_guid = guid;
            return false;
        }
        *budget -= 1;
        crate::mail::insert_letter(
            ctx,
            Letter {
                recipient_guid: guid,
                sender: MailSender::Creature(mailing.sender_entry),
                subject: String::new(),
                body: body.clone(),
                money: 0,
                cod: 0,
                item,
                mail_template_id: mailing.mail_template_id,
                check_flags: CHECK_MASK_RETURNED,
                deliver_micros: 0,
            },
        );
    }
    true
}

fn rewarded(ctx: &ReducerContext, character_guid: u64, quest_entry: u32) -> bool {
    ctx.db
        .game_character_quest()
        .by_character_quest()
        .filter((character_guid, quest_entry))
        .any(|row| row.reward_recorded(ctx))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HUMAN: u8 = 1;
    const ORC: u8 = 2;

    fn mail(race_mask: u32, quest_entry: u32) -> GameEventMailing {
        GameEventMailing {
            mailing_id: 1,
            event_entry: 7,
            race_mask,
            quest_entry,
            mail_template_id: 9,
            sender_entry: 3,
            next_guid: 0,
            own_mark: 1_000_000_700,
            last_guid: 2_000_000_005,
        }
    }

    #[test]
    fn a_character_minted_after_the_start_is_not_mailed_but_one_from_another_range_may_be() {
        let range = Some((1_000_000_000, 1_000_000_000));
        let mailing = mail(1, 0);
        assert!(existed_at_start(&mailing, range, 1_000_000_700));
        assert!(!existed_at_start(&mailing, range, 1_000_003_001));
        assert!(existed_at_start(&mailing, range, 2_000_000_005));
        assert!(!existed_at_start(&mailing, range, 2_000_000_006));
        assert!(!existed_at_start(&mailing, None, 1_000_003_001));
    }

    #[test]
    fn an_event_mails_when_it_starts_and_again_only_after_it_stopped() {
        assert_eq!(edge(GameEventState::Active, false), Some(Edge::Start));
        assert_eq!(edge(GameEventState::Active, true), None);
        assert_eq!(edge(GameEventState::Inactive, true), Some(Edge::Stop));
        assert_eq!(edge(GameEventState::Inactive, false), None);
    }

    #[test]
    fn an_unscheduled_event_stops_once_started_and_never_starts() {
        assert_eq!(edge(GameEventState::Unscheduled, false), None);
        assert_eq!(edge(GameEventState::Unscheduled, true), Some(Edge::Stop));
    }

    #[test]
    fn a_mail_goes_to_the_races_of_its_mask_who_were_rewarded_its_quest() {
        assert!(addressed_to(&mail(0b1, 0), HUMAN, || false));
        assert!(!addressed_to(&mail(0b1, 0), ORC, || true));
        assert!(addressed_to(&mail(0xff, 8), ORC, || true));
        assert!(!addressed_to(&mail(0xff, 8), ORC, || false));
        assert!(!addressed_to(&mail(0, 0), HUMAN, || true));
    }
}
