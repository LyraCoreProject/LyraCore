//! Reward Letters: the letter a quest giver sends when a Character turns a quest in.
//!
//! The turn-in runs on the Home Shard and the mail plane is Realm-core, so no transaction spans
//! both. The turn-in files the letter as Mail Escrow in its own transaction, and the Gateway drives
//! that escrow to the mail plane as it drives a stalled send. The letter then commits with the
//! turn-in or not at all, and the escrow id makes the commit happen once.

use lyracore_shared::mail::MailSender;
use spacetimedb::{log, ReducerContext};

use crate::game_item_template;
use crate::items::ItemSnapshot;
use crate::mail_catalogue::{game_mail_loot, game_mail_template, game_quest_reward_mail};
use crate::mail_catalogue::{MailLoot, QuestRewardMail};

/// A Reward Letter as the catalogue describes it, ready to file.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct RewardLetter {
    pub sender: MailSender,
    pub mail_template_id: u32,
    /// The Mail Template's text.
    pub body: String,
    pub money: u32,
    pub item: ItemSnapshot,
    /// How long after the commit the letter arrives (`quest_template.RewMailDelaySecs`).
    pub delay_secs: u32,
}

/// The Reward Letter `reward` describes. The sender is the alternative creature a
/// `SCRIPT_COMMAND_SEND_MAIL` row names, else the quest ender (cmangos `ScriptMgr.cpp:2850-2866`,
/// `Mail.cpp:48-75`). `body` is the Mail Template's text. `loot` is the template's one item and
/// `max_durability` that item's template value; the letter carries it unworn. `Err` names the
/// catalogue row that is missing.
pub(crate) fn plan_reward_letter(
    reward: &QuestRewardMail,
    quest_ender: Option<MailSender>,
    body: Option<String>,
    loot: Option<&MailLoot>,
    max_durability: Option<u32>,
) -> Result<RewardLetter, String> {
    let sender = match reward.sender_creature_entry {
        0 => quest_ender.ok_or("the quest ender is neither a creature nor a gameobject")?,
        entry => MailSender::Creature(entry),
    };
    let body = body.ok_or_else(|| format!("no mail template {}", reward.mail_template_id))?;
    let item = match loot {
        None => ItemSnapshot::default(),
        Some(loot) => ItemSnapshot {
            entry: loot.item_entry,
            stack_count: loot.count,
            durability: max_durability
                .ok_or_else(|| format!("no item template {}", loot.item_entry))?,
            ..ItemSnapshot::default()
        },
    };
    Ok(RewardLetter {
        sender,
        mail_template_id: reward.mail_template_id,
        body,
        money: reward.money,
        item,
        delay_secs: reward.delay_secs,
    })
}

/// File the Reward Letter `quest_entry` sends, if it sends one, for `player_guid`, who just turned
/// the quest in at `giver_guid`. Call it in the turn-in's transaction. A quest with no reward mail
/// files nothing. A letter whose catalogue rows are missing logs an error and files nothing, and
/// the turn-in still succeeds: the importer refuses that data, so it is not the player's fault.
/// `Err` when the Shard can mint no escrow id; the turn-in then fails too, so no letter is lost.
pub(crate) fn file_reward_letter(
    ctx: &ReducerContext,
    player_guid: u64,
    giver_guid: u64,
    quest_entry: u32,
) -> Result<(), String> {
    let Some(reward) = ctx
        .db
        .game_quest_reward_mail()
        .quest_entry()
        .find(quest_entry)
    else {
        return Ok(());
    };
    let body = ctx
        .db
        .game_mail_template()
        .id()
        .find(reward.mail_template_id)
        .map(|t| t.body);
    let loot = ctx
        .db
        .game_mail_loot()
        .mail_template_id()
        .find(reward.mail_template_id);
    let max_durability = loot.as_ref().and_then(|loot| {
        ctx.db
            .game_item_template()
            .entry()
            .find(loot.item_entry)
            .map(|t| t.max_durability)
    });
    let quest_ender = crate::quest::quest_ender_mail_sender(ctx, giver_guid);
    let letter = match plan_reward_letter(&reward, quest_ender, body, loot.as_ref(), max_durability)
    {
        Ok(letter) => letter,
        Err(missing) => {
            log::error!(
                "quest {quest_entry}: no Reward Letter for {player_guid} ({missing}); the \
                 turn-in stands"
            );
            return Ok(());
        }
    };
    // The Shard's GUID Range is its own slot in the Realm-core registry, and a Gateway mints its
    // escrow ids from a slot of its own there, so no other writer can issue this id.
    let escrow_id = *crate::auth::reserve_guids(ctx, 1, u64::MAX)?.start();
    crate::mail_escrow::file_reward(ctx, escrow_id, player_guid, &letter)
}

#[cfg(feature = "debug_reducers")]
mod fixture {
    use spacetimedb::{ReducerContext, Table};

    use crate::game_world_entity;
    use crate::mail_catalogue::{game_mail_loot, game_mail_template, game_quest_reward_mail};
    use crate::mail_catalogue::{MailLoot, MailTemplate, QuestRewardMail};
    use crate::quest::{
        game_character_quest, game_creature_quest, game_quest_template, quest_role, CharacterQuest,
        CreatureQuest, QuestTemplate,
    };

    const PLAYER: u64 = 1;
    /// The seed chicken. The fixture clones it into a giver next to the player.
    const GIVER_SOURCE: u64 = (0xF130_u64 << 48) | ((620_u64) << 24) | 1;
    const GIVER: u64 = (0xF130_u64 << 48) | ((620_u64) << 24) | 0x00FF_FF01;
    const GIVER_ENTRY: u32 = 620;
    /// Shaped like 3645, Membership Card Renewal: one item after 86,400 s (`cdb:` quest_template,
    /// mail_loot_template). The item is the seed's Tempered Blade.
    const CARD_QUEST: u32 = 509_091;
    const CARD_TEMPLATE: u32 = 509_091;
    const CARD_ITEM: u32 = crate::seed::FIXTURE_BLADE;
    /// Shaped like 8728: creature 11811 sends 1,000,000 copper after 129,600 s
    /// (`cdb:` dbscripts_on_quest_end).
    const ELDER_QUEST: u32 = 509_092;
    const ELDER_TEMPLATE: u32 = 509_092;
    /// A quest with no reward mail.
    const PLAIN_QUEST: u32 = 509_093;

    /// Stage three completed quests for the seeded player 1 at a giver creature next to it: a
    /// card-renewal-shaped letter with an item, an 8728-shaped letter with copper from another
    /// creature, and a quest that sends nothing.
    #[spacetimedb::reducer]
    pub fn debug_stage_reward_letter_fixture(ctx: &ReducerContext) -> Result<(), String> {
        crate::helpers::require_operator(ctx)?;
        let entities = ctx.db.game_world_entity();
        let player = entities
            .guid()
            .find(PLAYER)
            .ok_or_else(|| "reward letter fixture player is not live".to_string())?;
        let mut giver = entities
            .guid()
            .find(GIVER_SOURCE)
            .ok_or_else(|| "reward letter fixture giver source is not live".to_string())?;
        entities.guid().delete(GIVER);
        giver.guid = GIVER;
        giver.x = player.x + 1.0;
        giver.y = player.y;
        giver.z = player.z;
        giver.map_id = player.map_id;
        giver.instance_id = player.instance_id;
        let (grid_x, grid_y) = lyracore_shared::spatial::grid_cell(giver.x, giver.y);
        giver.grid_x = grid_x;
        giver.grid_y = grid_y;
        giver.cell = lyracore_shared::spatial::grid_cell_id(grid_x, grid_y);
        entities.insert(giver);

        let templates = ctx.db.game_mail_template();
        let loot = ctx.db.game_mail_loot();
        let rewards = ctx.db.game_quest_reward_mail();
        for (template, body) in [
            (CARD_TEMPLATE, "Your membership card, $n."),
            (ELDER_TEMPLATE, "The elders thank you, $n."),
        ] {
            templates.id().delete(template);
            templates.insert(MailTemplate {
                id: template,
                body: body.to_string(),
            });
        }
        loot.mail_template_id().delete(CARD_TEMPLATE);
        loot.insert(MailLoot {
            mail_template_id: CARD_TEMPLATE,
            item_entry: CARD_ITEM,
            count: 1,
        });
        for row in [
            QuestRewardMail {
                quest_entry: CARD_QUEST,
                mail_template_id: CARD_TEMPLATE,
                delay_secs: 86_400,
                sender_creature_entry: 0,
                money: 0,
            },
            QuestRewardMail {
                quest_entry: ELDER_QUEST,
                mail_template_id: ELDER_TEMPLATE,
                delay_secs: 129_600,
                sender_creature_entry: 11_811,
                money: 1_000_000,
            },
        ] {
            rewards.quest_entry().delete(row.quest_entry);
            rewards.insert(row);
        }

        for (relation, quest) in [
            (5_090_911_u64, CARD_QUEST),
            (5_090_921, ELDER_QUEST),
            (5_090_931, PLAIN_QUEST),
        ] {
            ctx.db.game_quest_template().entry().delete(quest);
            ctx.db.game_quest_template().insert(QuestTemplate {
                entry: quest,
                min_level: 0,
                quest_level: 1,
                title: format!("Reward letter fixture {quest}"),
                reward_money: 0,
                reward_xp: 10,
                prev_quest_id: 0,
                required_races: 0,
                required_classes: 0,
                zone_or_sort: 12,
                rew_rep_faction_1: 0,
                rew_rep_value_1: 0,
                rew_rep_faction_2: 0,
                rew_rep_value_2: 0,
                src_item: 0,
                src_item_count: 0,
                repeatable: false,
                next_quest_id: 0,
                limit_time: 0,
                reward_money_max_level: 0,
                quest_type: 0,
            });
            let relations = ctx.db.game_creature_quest();
            relations.id().delete(relation);
            relations.insert(CreatureQuest {
                id: relation,
                creature_entry: GIVER_ENTRY,
                quest_entry: quest,
                role: quest_role::END,
            });
            let log = ctx.db.game_character_quest();
            for row in log
                .by_character()
                .filter(&PLAYER)
                .filter(|row| row.quest_entry == quest)
                .collect::<Vec<_>>()
            {
                log.id().delete(row.id);
            }
            log.insert(CharacterQuest {
                id: 0,
                character_guid: PLAYER,
                owner_identity: player.owner_identity,
                quest_entry: quest,
                counts: Vec::new(),
                rewarded: false,
                deadline_micros: 0,
                failed: false,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reward(mail_template_id: u32, sender_creature_entry: u32, money: u32) -> QuestRewardMail {
        QuestRewardMail {
            quest_entry: 3645,
            mail_template_id,
            delay_secs: 86_400,
            sender_creature_entry,
            money,
        }
    }

    /// Quest 3645, Membership Card Renewal: template 99 attaches item 11423, count 1, and the quest
    /// ender sends it after 86,400 s (`cdb:` quest_template, mail_loot_template).
    #[test]
    fn the_quest_ender_sends_the_template_text_and_its_one_item() {
        let loot = MailLoot {
            mail_template_id: 99,
            item_entry: 11_423,
            count: 1,
        };
        let letter = plan_reward_letter(
            &reward(99, 0, 0),
            Some(MailSender::Creature(7_802)),
            Some("Your card, $n.".into()),
            Some(&loot),
            Some(0),
        )
        .expect("every row is present");
        assert_eq!(
            letter,
            RewardLetter {
                sender: MailSender::Creature(7_802),
                mail_template_id: 99,
                body: "Your card, $n.".into(),
                money: 0,
                item: ItemSnapshot {
                    entry: 11_423,
                    stack_count: 1,
                    ..ItemSnapshot::default()
                },
                delay_secs: 86_400,
            }
        );
    }

    /// Quest 8728: `dbscripts_on_quest_end` names creature 11811 and 1,000,000 copper, and the
    /// template carries no item (`cdb:`).
    #[test]
    fn a_script_sender_replaces_the_quest_ender_and_brings_its_copper() {
        let letter = plan_reward_letter(
            &reward(123, 11_811, 1_000_000),
            Some(MailSender::Creature(15_192)),
            Some("From the front.".into()),
            None,
            None,
        )
        .expect("every row is present");
        assert_eq!(letter.sender, MailSender::Creature(11_811));
        assert_eq!(letter.money, 1_000_000);
        assert!(letter.item.is_empty());
    }

    #[test]
    fn a_gameobject_quest_ender_sends_as_a_gameobject() {
        let letter = plan_reward_letter(
            &reward(99, 0, 0),
            Some(MailSender::Gameobject(176_582)),
            Some("A note.".into()),
            None,
            None,
        )
        .expect("every row is present");
        assert_eq!(letter.sender, MailSender::Gameobject(176_582));
    }

    #[test]
    fn a_letter_with_a_missing_catalogue_row_is_not_filed() {
        let loot = MailLoot {
            mail_template_id: 99,
            item_entry: 11_423,
            count: 1,
        };
        let ender = Some(MailSender::Creature(7_802));
        assert_eq!(
            plan_reward_letter(&reward(99, 0, 0), ender, None, None, None),
            Err("no mail template 99".to_string())
        );
        assert_eq!(
            plan_reward_letter(
                &reward(99, 0, 0),
                ender,
                Some("Your card.".into()),
                Some(&loot),
                None
            ),
            Err("no item template 11423".to_string())
        );
        assert!(plan_reward_letter(
            &reward(99, 0, 0),
            None,
            Some("Your card.".into()),
            None,
            None
        )
        .is_err());
    }

    /// A Shard mints from slot n, `[n * 1e9, (n + 1) * 1e9)`. A Gateway mints escrow ids from slot
    /// 10,000 or higher, so the ids of a Shard below slot 10,000 lie under every Gateway's.
    #[test]
    fn a_shard_escrow_id_lies_below_every_gateway_escrow_range() {
        assert_eq!(
            10_000 * crate::realm_core::GUID_RANGE_SIZE,
            10_000_000_000_000
        );
        for gateway in ["mail-escrow:gateway-a:00", "mail-escrow:gateway-b:ff"] {
            assert!(lyracore_shared::mail::escrow_range_mark(gateway) >= 10_000_000_000_000);
        }
    }
}
