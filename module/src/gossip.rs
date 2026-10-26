use lyracore_shared::constants::{gossip_option, npc_flags, MIN_TALENT_LEVEL};
use lyracore_shared::gossip::{self, CharacterFacts, Condition, ConditionKind, QuestState};
use spacetimedb::{table, ReducerContext};

use crate::creatures::{
    game_creature_gossip_menu_override, game_gossip_menu_profile, game_gossip_menu_profile_option,
    game_gossip_option,
};
use crate::items::{game_item_template, game_npc_vendor};
use crate::quest::{game_character_quest, game_quest_template};

#[table(accessor = game_gossip_condition, public)]
pub struct GossipCondition {
    #[primary_key]
    pub condition_id: u32,
    pub kind: i32,
    pub value1: u32,
    pub value2: u32,
    pub value3: u32,
    pub value4: u32,
    pub flags: u8,
}

/// `source_menu_id` is 0 for the entry menu, or the active profile id.
pub(crate) fn select_gate(
    ctx: &ReducerContext,
    character_guid: u64,
    npc_guid: u64,
    option_row_id: u32,
    source_menu_id: u32,
    selected_action: u32,
) -> Result<(), String> {
    let (actor, npc) = if selected_action == gossip_option::SPIRITHEALER {
        crate::ghost::spirit_healer_gate(ctx, character_guid, npc_guid)?
    } else {
        crate::items::npc_interaction_gate(
            ctx,
            character_guid,
            npc_guid,
            u32::MAX,
            "gossip creature",
            "target has no gossip",
            "use gossip",
        )?
    };
    let active_menu = active_menu(ctx, &npc);
    if source_menu_id != active_menu.as_ref().map_or(0, |menu| menu.menu_id) {
        return Err("gossip menu has changed".into());
    }
    if !service_available(ctx, &actor, &npc, selected_action) {
        return Err("gossip service is no longer available".into());
    }
    // u32::MAX denotes a synthesized service line without an imported row.
    if option_row_id == u32::MAX {
        if !synthesized_service_matches(
            ctx,
            &npc,
            active_menu.map(|menu| menu.menu_id),
            selected_action,
        ) {
            return Err("gossip service is no longer available".into());
        }
        return Ok(());
    }
    let (action, kind, value1, value2) = if let Some(menu) = active_menu {
        ctx.db
            .game_gossip_menu_profile_option()
            .row_id()
            .find(option_row_id)
            .filter(|option| option.menu_id == menu.menu_id)
            .map(|option| {
                (
                    option.action,
                    option.cond_type,
                    option.cond_value1,
                    option.cond_value2,
                )
            })
    } else {
        ctx.db
            .game_gossip_option()
            .row_id()
            .find(option_row_id)
            .filter(|option| option.entry == npc.entry)
            .map(|option| {
                (
                    option.action,
                    option.cond_type,
                    option.cond_value1,
                    option.cond_value2,
                )
            })
    }
    .ok_or_else(|| "gossip option does not belong to this creature's active menu".to_string())?;
    if action != selected_action {
        return Err("gossip option action has changed".into());
    }
    condition_gate(ctx, character_guid, actor.level, (kind, value1, value2))
}

pub(crate) fn active_menu(
    ctx: &ReducerContext,
    npc: &crate::WorldEntity,
) -> Option<crate::creatures::GossipMenuProfile> {
    ctx.db
        .game_creature_gossip_menu_override()
        .creature_guid()
        .find(npc.guid)
        .filter(|menu| menu.map_id == npc.map_id && menu.instance_id == npc.instance_id)
        .and_then(|menu| {
            ctx.db
                .game_gossip_menu_profile()
                .menu_id()
                .find(menu.menu_id)
        })
}

fn condition_gate(
    ctx: &ReducerContext,
    character_guid: u64,
    level: u32,
    (kind, value1, value2): (u32, u32, u32),
) -> Result<(), String> {
    let character = crate::helpers::character_by_guid(ctx, character_guid)
        .ok_or_else(|| "gossip Character is unavailable".to_string())?;
    let result = gossip::evaluate_option(
        kind,
        value1,
        value2,
        CharacterFacts {
            level,
            race: character.race,
            class: character.class,
        },
        |quest_entry| quest_state(ctx, character_guid, quest_entry),
        |id| {
            ctx.db
                .game_gossip_condition()
                .condition_id()
                .find(id)
                .map(|row| Condition {
                    kind: ConditionKind::from_code(row.kind),
                    values: [row.value1, row.value2, row.value3, row.value4],
                    flags: row.flags,
                })
        },
    );
    match result {
        Ok(true) => Ok(()),
        Ok(false) => Err("gossip option is no longer available".into()),
        Err(reason) => Err(format!(
            "gossip option condition is unavailable: {reason:?}"
        )),
    }
}

fn synthesized_service_matches(
    ctx: &ReducerContext,
    npc: &crate::WorldEntity,
    menu_id: Option<u32>,
    selected_action: u32,
) -> bool {
    if !matches!(
        selected_action,
        gossip_option::VENDOR | gossip_option::INNKEEPER
    ) {
        return false;
    }
    let has_authored_service = if let Some(menu_id) = menu_id {
        ctx.db
            .game_gossip_menu_profile_option()
            .by_menu()
            .filter(&menu_id)
            .any(|option| option.action == selected_action)
    } else {
        ctx.db
            .game_gossip_option()
            .by_entry()
            .filter(&npc.entry)
            .any(|option| option.action == selected_action)
    };
    !has_authored_service
}

fn service_available(
    ctx: &ReducerContext,
    actor: &crate::WorldEntity,
    npc: &crate::WorldEntity,
    selected_action: u32,
) -> bool {
    match selected_action {
        gossip_option::VENDOR => {
            npc.npc_flags & npc_flags::VENDOR != 0
                && ctx
                    .db
                    .game_npc_vendor()
                    .by_vendor()
                    .filter(&npc.entry)
                    .any(|stock| {
                        ctx.db
                            .game_item_template()
                            .entry()
                            .find(stock.item_entry)
                            .is_some()
                    })
        }
        gossip_option::INNKEEPER => npc.npc_flags & npc_flags::INNKEEPER != 0,
        gossip_option::TRAINER | gossip_option::UNLEARNTALENTS => {
            (selected_action != gossip_option::UNLEARNTALENTS
                || actor.level >= u32::from(MIN_TALENT_LEVEL))
                && crate::trainer::validate_trainer_interaction(ctx, actor, npc.guid).is_ok()
        }
        gossip_option::TAXI => npc.npc_flags & npc_flags::TAXI != 0,
        gossip_option::BANKER => npc.npc_flags & npc_flags::BANKER != 0,
        gossip_option::PETITIONER => npc.npc_flags & npc_flags::PETITIONER != 0,
        gossip_option::TABARDDESIGNER => npc.npc_flags & npc_flags::TABARDDESIGNER != 0,
        _ => true,
    }
}

fn quest_state(ctx: &ReducerContext, character_guid: u64, quest_entry: u32) -> QuestState {
    let Some(template) = ctx.db.game_quest_template().entry().find(quest_entry) else {
        return QuestState::Missing;
    };
    let Some(quest) = ctx
        .db
        .game_character_quest()
        .by_character_quest()
        .filter((character_guid, quest_entry))
        .next()
    else {
        return QuestState::Absent;
    };
    if quest.rewarded {
        if template.repeatable {
            QuestState::Absent
        } else {
            QuestState::Rewarded
        }
    } else if quest.failed {
        QuestState::Failed
    } else if crate::quest::quest_is_complete(ctx, &quest) {
        QuestState::Complete
    } else {
        QuestState::Incomplete
    }
}
