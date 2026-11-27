// vmangos `GameEventMgr::SendEventMails`; read docs/schema.md#game-events first.

use std::collections::BTreeMap;

use lyracore_shared::mail::{MailSender, CHECK_MASK_RETURNED};
use lyracore_shared::race_class::{mask_admits, ZeroMask};
use spacetimedb::{log, table, ReducerContext, Table};

use super::GameEventState;
use crate::game_character;
use crate::mail::Letter;
use crate::quest::game_character_quest;

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

crate::outside_table_catalog!(game_event_mail, game_event_started);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edge {
    Start,
    Stop,
}

/// The edge an event crosses from what this Shard last followed. An unscheduled event crosses
/// none, as vmangos `GameEventMgr::Update` skips a disabled event.
fn edge(state: GameEventState, started: bool) -> Option<Edge> {
    match (state, started) {
        (GameEventState::Active, false) => Some(Edge::Start),
        (GameEventState::Inactive, true) => Some(Edge::Stop),
        _ => None,
    }
}

/// Whether a Character of `race` receives the mail; `rewarded` tells whether the Character's reward
/// of the mail's quest is recorded.
fn addressed_to(mail: &GameEventMail, race: u8, rewarded: impl FnOnce() -> bool) -> bool {
    mask_admits(mail.race_mask, race, ZeroMask::AdmitsNone) && (mail.quest_entry == 0 || rewarded())
}

/// Sends the mail of each event that started since this Shard last followed it, as vmangos
/// `ApplyNewEvent` does; a stop lets the event's next start mail again.
pub(super) fn follow(ctx: &ReducerContext, states: &BTreeMap<u32, GameEventState>) {
    let started = ctx.db.game_event_started();
    for (&event_entry, &state) in states {
        let Some(edge) = edge(state, started.event_entry().find(event_entry).is_some()) else {
            continue;
        };
        match edge {
            Edge::Start => {
                started.insert(GameEventStarted { event_entry });
                send(ctx, event_entry);
            }
            Edge::Stop => {
                started.event_entry().delete(event_entry);
            }
        }
    }
}

/// Mails each Character this Shard holds that the event's mail addresses; `insert_letter` routes a
/// letter for one behind the Transfer Gate to its new home.
fn send(ctx: &ReducerContext, event_entry: u32) {
    let mails: Vec<GameEventMail> = ctx
        .db
        .game_event_mail()
        .by_event()
        .filter(event_entry)
        .collect();
    if mails.is_empty() {
        return;
    }
    let characters: Vec<(u64, u8)> = ctx
        .db
        .game_character()
        .iter()
        .map(|character| (character.guid, character.race))
        .collect();
    for mail in mails {
        let (body, item) = match crate::mail_reward::template_letter(ctx, mail.mail_template_id) {
            Ok(contents) => contents,
            Err(missing) => {
                log::error!(
                    "game event {event_entry}: mail {} not sent ({missing})",
                    mail.id
                );
                continue;
            }
        };
        for &(guid, race) in &characters {
            if !addressed_to(&mail, race, || rewarded(ctx, guid, mail.quest_entry)) {
                continue;
            }
            crate::mail::insert_letter(
                ctx,
                Letter {
                    recipient_guid: guid,
                    sender: MailSender::Creature(mail.sender_entry),
                    subject: String::new(),
                    body: body.clone(),
                    money: 0,
                    cod: 0,
                    item,
                    mail_template_id: mail.mail_template_id,
                    check_flags: CHECK_MASK_RETURNED,
                    deliver_micros: 0,
                },
            );
        }
    }
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

    fn mail(race_mask: u32, quest_entry: u32) -> GameEventMail {
        GameEventMail {
            id: 1,
            event_entry: 7,
            race_mask,
            quest_entry,
            mail_template_id: 9,
            sender_entry: 3,
        }
    }

    #[test]
    fn an_event_mails_when_it_starts_and_again_only_after_it_stopped() {
        assert_eq!(edge(GameEventState::Active, false), Some(Edge::Start));
        assert_eq!(edge(GameEventState::Active, true), None);
        assert_eq!(edge(GameEventState::Inactive, true), Some(Edge::Stop));
        assert_eq!(edge(GameEventState::Inactive, false), None);
    }

    #[test]
    fn an_unscheduled_event_neither_starts_nor_stops() {
        assert_eq!(edge(GameEventState::Unscheduled, false), None);
        assert_eq!(edge(GameEventState::Unscheduled, true), None);
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
