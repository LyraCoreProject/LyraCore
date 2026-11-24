use crate::actor::{ActionRefusal, ActionRefusalKind};
use crate::npc_interaction::NpcService;
use crate::quest::*;
use crate::{game_creature_template, game_item_template};
use lyracore_shared::quest::{AcceptanceRefusal, StartSighting};
use spacetimedb::{ConnectionId, ReducerContext};

use super::{Rollback, Unhandled};
use lyracore_protocol::{
    codec,
    messages::vanilla::opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
    world::Packet,
};

pub(super) fn quest_detail_view(
    ctx: &ReducerContext,
    quest_id: u32,
) -> Option<lyracore_protocol::codec::QuestDetailView> {
    use lyracore_protocol::codec::QuestDetailView;
    let tmpl = ctx.db.game_quest_template().entry().find(quest_id)?;
    let text = ctx.db.game_quest_text().quest_entry().find(quest_id);
    let details = text.as_ref().map(|t| t.details.clone()).unwrap_or_default();
    let offer_reward_text = text
        .as_ref()
        .map(|t| t.offer_reward_text.clone())
        .unwrap_or_default();
    let request_items_text = text
        .as_ref()
        .map(|t| t.request_items_text.clone())
        .unwrap_or_default();
    let imported_objectives = text.map(|t| t.objectives).unwrap_or_default();
    let mut raw_objs: Vec<(u8, u32, u32, u32, u32)> = ctx
        .db
        .game_quest_objective()
        .by_quest()
        .filter(quest_id)
        .filter_map(|o| {
            let slot = match o.kind {
                objective_kind::KILL_CREATURE => (o.target_entry, o.required_count, 0u32, 0u32),
                objective_kind::COLLECT_ITEM => (0, 0, o.target_entry, o.required_count),
                objective_kind::USE_GAMEOBJECT => {
                    (0u32.wrapping_sub(o.target_entry), o.required_count, 0, 0)
                }
                _ => return None,
            };
            Some((o.obj_index, slot.0, slot.1, slot.2, slot.3))
        })
        .collect();
    raw_objs.sort_by_key(|(i, _, _, _, _)| *i);
    Some(QuestDetailView {
        quest_id,
        quest_level: tmpl.quest_level,
        zone_or_sort: tmpl.zone_or_sort,
        title: tmpl.title.clone(),
        details,
        objectives_text: synthesized_objectives(ctx, quest_id, imported_objectives),
        offer_reward_text,
        request_items_text,
        money_reward: lyracore_shared::quest::offered_money(tmpl.reward_money, tmpl.required_money),
        next_quest_id: tmpl.next_quest_id,
        max_level_money_reward: tmpl.reward_money_max_level,
        flags: ctx
            .db
            .game_quest_flags()
            .quest_entry()
            .find(quest_id)
            .map(|quest| quest.flags)
            .unwrap_or(0),
        reward_spell: tmpl.reward_spell,
        src_item: tmpl.src_item,
        rewards: quest_rewards(ctx, quest_id),
        choice_rewards: quest_choice_rewards(ctx, quest_id),
        required_items: required_items(ctx, &raw_objs),
        emotes: quest_emotes(ctx, quest_id),
        objectives: raw_objs
            .into_iter()
            .map(|(_, a, b, c, d)| (a, b, c, d))
            .collect(),
    })
}

// The XP and signed money of the turn-in, read at the Character's level before it is granted.
fn turn_in_payout_of(ctx: &ReducerContext, guid: u64, quest: u32) -> Option<(u32, i32)> {
    use crate::game_world_entity;
    let level = ctx.db.game_world_entity().guid().find(guid)?.level;
    let tmpl = ctx.db.game_quest_template().entry().find(quest)?;
    let payout = turn_in_payout(ctx, level, &tmpl);
    let money = tmpl.reward_money.wrapping_add(payout.money);
    Some((
        payout.xp.unwrap_or(0),
        lyracore_shared::quest::offered_money(money, tmpl.required_money),
    ))
}

fn quest_emotes(ctx: &ReducerContext, quest_id: u32) -> codec::QuestEmotes {
    let Some(row) = ctx.db.game_quest_emote().quest_entry().find(quest_id) else {
        return codec::QuestEmotes::default();
    };
    let offer_reward = [
        (row.offer_reward_delay_1, row.offer_reward_emote_1),
        (row.offer_reward_delay_2, row.offer_reward_emote_2),
        (row.offer_reward_delay_3, row.offer_reward_emote_3),
        (row.offer_reward_delay_4, row.offer_reward_emote_4),
    ]
    .into_iter()
    .take_while(|&(_, emote)| emote != 0)
    .collect();
    codec::QuestEmotes {
        incomplete: row.incomplete_emote,
        complete: row.complete_emote,
        offer_reward,
    }
}

fn required_items(
    ctx: &ReducerContext,
    objectives: &[(u8, u32, u32, u32, u32)],
) -> Vec<lyracore_protocol::codec::QuestRewardView> {
    objectives
        .iter()
        .filter(|&&(_, _, _, item, _)| item != 0)
        .map(|&(_, _, _, item_entry, count)| {
            let display_id = ctx
                .db
                .game_item_template()
                .entry()
                .find(item_entry)
                .map(|t| t.display_id)
                .unwrap_or(0);
            lyracore_protocol::codec::QuestRewardView {
                item_entry,
                count,
                display_id,
            }
        })
        .collect()
}

fn quest_rewards(
    ctx: &ReducerContext,
    quest_id: u32,
) -> Vec<lyracore_protocol::codec::QuestRewardView> {
    ctx.db
        .game_quest_reward_item()
        .by_quest()
        .filter(quest_id)
        .map(|r| {
            let display_id = ctx
                .db
                .game_item_template()
                .entry()
                .find(r.item_entry)
                .map(|t| t.display_id)
                .unwrap_or(0);
            lyracore_protocol::codec::QuestRewardView {
                item_entry: r.item_entry,
                count: r.count,
                display_id,
            }
        })
        .collect()
}

fn quest_choice_rewards(
    ctx: &ReducerContext,
    quest_id: u32,
) -> Vec<lyracore_protocol::codec::QuestRewardView> {
    let mut rows: Vec<_> = ctx
        .db
        .game_quest_reward_choice()
        .by_quest()
        .filter(quest_id)
        .collect();
    rows.sort_by_key(|r| r.choice_index);
    rows.into_iter()
        .map(|r| {
            let display_id = ctx
                .db
                .game_item_template()
                .entry()
                .find(r.item_entry)
                .map(|t| t.display_id)
                .unwrap_or(0);
            lyracore_protocol::codec::QuestRewardView {
                item_entry: r.item_entry,
                count: r.count,
                display_id,
            }
        })
        .collect()
}

fn synthesized_objectives(ctx: &ReducerContext, quest_id: u32, imported: String) -> String {
    if !imported.trim().is_empty() {
        return imported;
    }
    let mut objs: Vec<(u8, String)> = ctx
        .db
        .game_quest_objective()
        .by_quest()
        .filter(quest_id)
        .map(|o| {
            let text = match o.kind {
                objective_kind::KILL_CREATURE => {
                    let name = ctx
                        .db
                        .game_creature_template()
                        .entry()
                        .find(o.target_entry)
                        .map(|c| c.name.clone())
                        .unwrap_or_default();
                    format!("{name} slain: 0/{}", o.required_count)
                }
                objective_kind::COLLECT_ITEM => {
                    let name = ctx
                        .db
                        .game_item_template()
                        .entry()
                        .find(o.target_entry)
                        .map(|t| t.name.clone())
                        .unwrap_or_default();
                    format!("{name}: 0/{}", o.required_count)
                }
                _ => String::new(),
            };
            (o.obj_index, text)
        })
        .filter(|(_, t)| !t.is_empty())
        .collect();
    objs.sort_by_key(|(i, _)| *i);
    objs.into_iter()
        .map(|(_, s)| s)
        .collect::<Vec<_>>()
        .join("\n")
}

fn packet(message: ServerOpcodeMessage) -> Result<Packet, String> {
    lyracore_protocol::world::encode(&message).map_err(|error| error.to_string())
}

fn details(ctx: &ReducerContext, giver: u64, quest: u32) -> Vec<Packet> {
    quest_detail_view(ctx, quest)
        .map(|detail| {
            let (opcode, body) = codec::build_quest_details_raw(giver, &detail);
            Packet { opcode, body }
        })
        .into_iter()
        .collect()
}

fn next_in_chain(ctx: &ReducerContext, giver: u64, quest: u32) -> Option<u32> {
    let next = ctx
        .db
        .game_quest_template()
        .entry()
        .find(quest)?
        .next_quest_id;
    giver_relations(ctx, giver)
        .contains(&QuestRelation {
            quest_entry: next,
            role: quest_role::START,
        })
        .then_some(next)
}

pub(super) fn evaluations(
    ctx: &ReducerContext,
    giver: u64,
    guid: u64,
) -> Vec<codec::GiverQuestEval> {
    use crate::game_world_entity;
    let Some(actor) = ctx
        .db
        .game_world_entity()
        .guid()
        .find(guid)
        .filter(|e| !e.dead)
    else {
        return Vec::new();
    };
    giver_relations(ctx, giver)
        .into_iter()
        .filter_map(
            |QuestRelation {
                 quest_entry: entry,
                 role,
             }| {
                let template = ctx.db.game_quest_template().entry().find(entry)?;
                let row = character_quest_row(ctx, guid, entry);
                let active = row.as_ref().is_some_and(|row| row.is_active());
                let (startable, sighting) = if role == quest_role::START {
                    start_offer(ctx, &actor, &template)
                } else {
                    (false, StartSighting::Hidden)
                };
                Some(codec::GiverQuestEval {
                    quest_id: entry,
                    title: template.title.clone(),
                    level: template.quest_level,
                    role,
                    startable,
                    sighting,
                    active,
                    complete: active && row.as_ref().is_some_and(|row| quest_is_complete(ctx, row)),
                })
            },
        )
        .collect()
}

// vmangos shows no mark over a creature whose reaction to the Character is hostile.
fn giver_status(
    ctx: &ReducerContext,
    giver: u64,
    guid: u64,
) -> lyracore_protocol::messages::vanilla::QuestGiverStatus {
    use crate::game_world_entity;
    let hostile = lyracore_shared::guid::is_creature(giver)
        && ctx
            .db
            .game_world_entity()
            .guid()
            .find(giver)
            .zip(ctx.db.game_world_entity().guid().find(guid))
            .is_some_and(|(npc, actor)| crate::reputation::npc_is_hostile(ctx, &npc, &actor));
    if hostile {
        lyracore_protocol::messages::vanilla::QuestGiverStatus::None
    } else {
        codec::quest_giver_status(&evaluations(ctx, giver, guid))
    }
}

pub(super) fn menu(ctx: &ReducerContext, giver: u64, guid: u64) -> Result<Vec<Packet>, String> {
    let evals = evaluations(ctx, giver, guid);
    let items = codec::quest_menu_items(&evals);
    let detail = (items.len() == 1)
        .then(|| quest_detail_view(ctx, items[0].quest_id))
        .flatten();
    let Some(detail) = detail else {
        return Ok(vec![packet(
            ServerOpcodeMessage::SMSG_QUESTGIVER_QUEST_LIST(Box::new(codec::build_quest_list(
                giver,
                "Greetings.",
                &evals,
            ))),
        )?]);
    };
    let turn_in = evals.iter().find(|eval| {
        eval.quest_id == detail.quest_id && eval.role == quest_role::END && eval.active
    });
    match turn_in {
        None => Ok(details(ctx, giver, detail.quest_id)),
        Some(eval) => Ok(vec![request_items_or_offer_reward(
            giver,
            &detail,
            eval.complete,
            true,
        )?]),
    }
}

fn offer_reward(giver: u64, detail: &codec::QuestDetailView) -> Result<Packet, String> {
    packet(ServerOpcodeMessage::SMSG_QUESTGIVER_OFFER_REWARD(Box::new(
        codec::build_offer_reward(giver, detail),
    )))
}

fn quest_is_ready_to_reward(ctx: &ReducerContext, giver: u64, guid: u64, quest: u32) -> bool {
    use crate::game_world_entity;
    ctx.db
        .game_world_entity()
        .guid()
        .find(guid)
        .is_some_and(|actor| !actor.dead)
        && giver_relations(ctx, giver).contains(&QuestRelation {
            quest_entry: quest,
            role: quest_role::END,
        })
        && character_quest_row(ctx, guid, quest)
            .is_some_and(|row| row.is_active() && quest_is_complete(ctx, &row))
}

// Request-items shows only when the quest has request text and either needs items or is not done.
fn request_items_or_offer_reward(
    giver: u64,
    detail: &codec::QuestDetailView,
    complete: bool,
    close_on_cancel: bool,
) -> Result<Packet, String> {
    if detail.request_items_text.is_empty() || (detail.required_items.is_empty() && complete) {
        return offer_reward(giver, detail);
    }
    packet(ServerOpcodeMessage::SMSG_QUESTGIVER_REQUEST_ITEMS(
        Box::new(codec::build_request_items(
            giver,
            detail,
            complete,
            close_on_cancel,
        )),
    ))
}

fn complete_quest_screen(
    ctx: &ReducerContext,
    giver: u64,
    guid: u64,
    quest: u32,
) -> Result<Vec<Packet>, String> {
    let Some(detail) = quest_detail_view(ctx, quest) else {
        return Ok(Vec::new());
    };
    let complete = quest_is_ready_to_reward(ctx, giver, guid, quest);
    Ok(vec![request_items_or_offer_reward(
        giver, &detail, complete, false,
    )?])
}

fn request_reward_screen(
    ctx: &ReducerContext,
    giver: u64,
    guid: u64,
    quest: u32,
) -> Result<Vec<Packet>, String> {
    let Some(detail) = quest_detail_view(ctx, quest) else {
        return Ok(Vec::new());
    };
    if !quest_is_ready_to_reward(ctx, giver, guid, quest) {
        return Ok(Vec::new());
    }
    Ok(vec![offer_reward(giver, &detail)?])
}

// Storage refusals send the quest-failed notice; only a quest this giver ends is re-offered.
fn refused_turn_in_screen(
    ctx: &ReducerContext,
    giver: u64,
    guid: u64,
    quest: u32,
    refusal: &ActionRefusal,
) -> Result<Vec<Packet>, String> {
    use crate::game_world_entity;
    use lyracore_protocol::messages::vanilla::QuestFailedReason as Reason;
    let alive = ctx
        .db
        .game_world_entity()
        .guid()
        .find(guid)
        .is_some_and(|actor| !actor.dead);
    let ends_quest = giver_relations(ctx, giver).contains(&QuestRelation {
        quest_entry: quest,
        role: quest_role::END,
    });
    let Some(detail) = quest_detail_view(ctx, quest).filter(|_| alive && ends_quest) else {
        return Ok(Vec::new());
    };
    let failed = |reason| {
        ServerOpcodeMessage::SMSG_QUESTGIVER_QUEST_FAILED(codec::build_questgiver_quest_failed(
            quest, reason,
        ))
    };
    let notice = match refusal.kind {
        ActionRefusalKind::InventoryFull => Some(failed(Reason::QuestFailedInventoryFull)),
        ActionRefusalKind::DuplicateItem => Some(failed(Reason::QuestFailedDuplicateItem)),
        _ => None,
    }
    .map(packet)
    .transpose()?;
    Ok(notice
        .into_iter()
        .chain([offer_reward(giver, &detail)?])
        .collect())
}

fn accept_refusal(refusal: &AcceptQuestRefusal) -> Vec<Packet> {
    use lyracore_protocol::messages::vanilla::QuestFailedReason as Reason;
    let message = match refusal {
        AcceptQuestRefusal::LogFull => Some(ServerOpcodeMessage::SMSG_QUESTLOG_FULL),
        AcceptQuestRefusal::Acceptance { refusal, .. } => {
            let reason = match refusal {
                AcceptanceRefusal::AboveMaxLevel => None,
                AcceptanceRefusal::Race => Some(Reason::QuestFailedWrongRace),
                AcceptanceRefusal::AlreadyTaken => Some(Reason::QuestAlreadyOn),
                AcceptanceRefusal::Level
                | AcceptanceRefusal::Class
                | AcceptanceRefusal::Prerequisite
                | AcceptanceRefusal::Skill
                | AcceptanceRefusal::Reputation => Some(Reason::DontHaveReq),
            };
            reason.map(|reason| {
                ServerOpcodeMessage::SMSG_QUESTGIVER_QUEST_INVALID(
                    codec::build_questgiver_quest_invalid(reason),
                )
            })
        }
        AcceptQuestRefusal::Refused(_) => None,
    };
    message
        .into_iter()
        .chain([ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE])
        .map(super::bounded)
        .collect()
}

pub(super) fn receive(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    request: ClientOpcodeMessage,
) -> Result<(), Unhandled> {
    let packets = match request {
        ClientOpcodeMessage::CMSG_QUESTGIVER_STATUS_QUERY(q) => vec![packet(
            ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(Box::new(codec::build_questgiver_status(
                q.guid.guid(),
                giver_status(ctx, q.guid.guid(), guid),
            ))),
        )?],
        ClientOpcodeMessage::CMSG_QUESTGIVER_HELLO(q) => {
            let actor = super::actor(ctx, guid)?;
            let giver = q.guid.guid();
            let refused = if lyracore_shared::guid::is_creature(giver) {
                crate::npc_interaction::serving_npc(ctx, &actor, giver, NpcService::QuestGiver)
                    .is_err()
            } else {
                use crate::game_world_entity;
                ctx.db
                    .game_world_entity()
                    .guid()
                    .find(giver)
                    .is_some_and(|npc| crate::reputation::npc_refuses_service(ctx, &npc, &actor))
            };
            if refused {
                return Ok(());
            }
            menu(ctx, giver, guid)?
        }
        ClientOpcodeMessage::CMSG_QUESTGIVER_QUERY_QUEST(q) => {
            use crate::game_item_instance;
            let giver = q.guid.guid();
            let related = giver_relations(ctx, giver)
                .iter()
                .any(|relation| relation.quest_entry == q.quest_id)
                || ctx
                    .db
                    .game_item_instance()
                    .guid()
                    .find(giver)
                    .filter(|item| item.owner_guid == guid)
                    .and_then(|item| ctx.db.game_item_template().entry().find(item.entry))
                    .is_some_and(|item| item.start_quest == q.quest_id);
            if related {
                details(ctx, giver, q.quest_id)
            } else {
                vec![packet(ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE)?]
            }
        }
        ClientOpcodeMessage::CMSG_QUEST_QUERY(q) => quest_detail_view(ctx, q.quest_id)
            .map(|detail| {
                let (opcode, body) = codec::build_quest_query_response_raw(&detail);
                Packet { opcode, body }
            })
            .into_iter()
            .collect(),
        ClientOpcodeMessage::CMSG_QUESTGIVER_ACCEPT_QUEST(q) => {
            super::actor(ctx, guid)
                .map_err(|refusal| AcceptQuestRefusal::from(String::from(refusal)))
                .and_then(|_| accept_offered_quest(ctx, guid, q.guid.guid(), q.quest_id))
                .map_err(|refusal| {
                    Rollback::replies(accept_refusal(&refusal), ActionRefusal::from(refusal))
                })?;
            vec![packet(ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE)?]
        }
        ClientOpcodeMessage::CMSG_QUESTGIVER_CANCEL => {
            vec![packet(ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE)?]
        }
        ClientOpcodeMessage::CMSG_QUESTGIVER_COMPLETE_QUEST(q) => {
            complete_quest_screen(ctx, q.guid.guid(), guid, q.quest_id)?
        }
        ClientOpcodeMessage::CMSG_QUESTGIVER_REQUEST_REWARD(q) => {
            request_reward_screen(ctx, q.guid.guid(), guid, q.quest_id)?
        }
        ClientOpcodeMessage::CMSG_QUESTGIVER_CHOOSE_REWARD(q) => {
            if q.reward >= QUEST_REWARD_CHOICES_COUNT {
                return Ok(());
            }
            let payout = turn_in_payout_of(ctx, guid, q.quest_id);
            let turned_in = super::actor(ctx, guid)
                .map_err(|refusal| ActionRefusal::from(String::from(refusal)))
                .and_then(|_| request_turn_in_quest(ctx, guid, q.guid.guid(), q.quest_id, q.reward))
                .and_then(|()| {
                    crate::mail_reward::file_reward_letter(ctx, guid, q.guid.guid(), q.quest_id)
                        .map_err(|refusal| ActionRefusal::from(String::from(refusal)))
                });
            if let Err(reason) = turned_in {
                let packets =
                    refused_turn_in_screen(ctx, q.guid.guid(), guid, q.quest_id, &reason)?;
                return Err(Rollback::replies(packets, reason).into());
            }
            let mut packets: Vec<Packet> = quest_detail_view(ctx, q.quest_id)
                .zip(payout)
                .map(|(detail, (xp, money))| {
                    packet(ServerOpcodeMessage::SMSG_QUESTGIVER_QUEST_COMPLETE(
                        Box::new(codec::build_quest_complete(&detail, xp, money)),
                    ))
                })
                .transpose()?
                .into_iter()
                .collect();
            if let Some(next) = next_in_chain(ctx, q.guid.guid(), q.quest_id) {
                packets.extend(details(ctx, q.guid.guid(), next));
            }
            packets
        }
        ClientOpcodeMessage::CMSG_QUESTLOG_SWAP_QUEST(q) => {
            super::actor(ctx, guid)?;
            if q.slot1 == q.slot2
                || usize::from(q.slot1) >= MAX_QUEST_LOG_SIZE
                || usize::from(q.slot2) >= MAX_QUEST_LOG_SIZE
            {
                return Ok(());
            }
            let before = super::view::quests(ctx, guid);
            let mut entries = quest_log_entries(ctx, guid);
            entries.swap(usize::from(q.slot1), usize::from(q.slot2));
            save_quest_log(ctx, guid, entries);
            send_log_changes(ctx, guid, &before, &super::view::quests(ctx, guid));
            Vec::new()
        }
        ClientOpcodeMessage::CMSG_QUESTLOG_REMOVE_QUEST(q) => {
            super::actor(ctx, guid)?;
            if let Some(slot) = super::view::quests(ctx, guid)
                .into_iter()
                .find(|slot| slot.slot == q.slot)
            {
                apply_abandon_quest(ctx, guid, slot.quest_id)?;
            }
            Vec::new()
        }
        ClientOpcodeMessage::CMSG_PUSHQUESTTOPARTY(q) => {
            super::actor(ctx, guid)?;
            apply_push_quest_to_party(ctx, guid, q.quest_id)?;
            Vec::new()
        }
        ClientOpcodeMessage::MSG_QUEST_PUSH_RESULT(answer) => {
            share::answer_quest_share(ctx, guid, answer.message.as_int());
            Vec::new()
        }
        other => return Err(Unhandled::Declined(other)),
    };
    for packet in packets {
        super::send_raw(ctx, connection, packet.opcode, packet.body, false);
    }
    Ok(())
}

pub(super) fn refresh(ctx: &ReducerContext, guid: u64) {
    if !super::has_world_session(ctx, guid) {
        return;
    }
    let slots = super::view::quests(ctx, guid);
    let (opcode, body) =
        codec::build_values_update_raw(guid, &codec::update_mask::full_quest_log_mask(&slots));
    super::send_raw_to_character(ctx, guid, opcode, body);
}

pub(crate) fn changed(
    ctx: &ReducerContext,
    old: Option<&CharacterQuest>,
    new: Option<&CharacterQuest>,
    slot: u8,
) {
    let Some(row) = new.or(old) else {
        return;
    };
    if !super::has_world_session(ctx, row.character_guid) {
        return;
    }
    if let Some((old, row)) = old.zip(new) {
        if old.counts == row.counts
            && old.rewarded == row.rewarded
            && old.failed == row.failed
            && old.deadline_micros == row.deadline_micros
        {
            return;
        }
        let objectives: Vec<_> = ctx
            .db
            .game_quest_objective()
            .by_quest()
            .filter(row.quest_entry)
            .map(|o| (o.kind, o.obj_index, o.target_entry, o.required_count))
            .collect();
        let mut messages: Vec<_> =
            codec::kill_progress_add_kills(row.quest_entry, &old.counts, &row.counts, &objectives)
                .into_iter()
                .map(|value| ServerOpcodeMessage::SMSG_QUESTUPDATE_ADD_KILL(Box::new(value)))
                .collect();
        if objectives.iter().any(|&(kind, index, _, required)| {
            kind == objective_kind::EXPLORE_AREATRIGGER
                && old.counts.get(index as usize).copied().unwrap_or(0) < required
                && row.counts.get(index as usize).copied().unwrap_or(0) >= required
        }) {
            messages.push(ServerOpcodeMessage::SMSG_QUESTUPDATE_COMPLETE(
                codec::build_questupdate_complete(row.quest_entry),
            ));
        }
        if !old.failed && row.failed {
            messages.push(if old.deadline_micros != 0 {
                ServerOpcodeMessage::SMSG_QUESTUPDATE_FAILEDTIMER(
                    codec::build_questupdate_failedtimer(row.quest_entry),
                )
            } else {
                ServerOpcodeMessage::SMSG_QUESTUPDATE_FAILED(
                    lyracore_protocol::messages::vanilla::SMSG_QUESTUPDATE_FAILED {
                        quest_id: row.quest_entry,
                    },
                )
            });
        }
        for message in messages {
            super::send_to_character(
                ctx,
                row.character_guid,
                message,
                super::EncodeFailure::Panic,
            );
        }
    }
    let after = super::view::quests(ctx, row.character_guid);
    let mut before = after.clone();
    before.retain(|slot| slot.quest_id != row.quest_entry);
    if let Some(old) = old.filter(|old| !old.rewarded && usize::from(slot) < MAX_QUEST_LOG_SIZE) {
        before.push(super::view::quest_slot(ctx, old, slot));
    }
    send_log_changes(ctx, row.character_guid, &before, &after);
}

fn send_log_changes(
    ctx: &ReducerContext,
    guid: u64,
    before: &[codec::update_mask::QuestLogSlot],
    after: &[codec::update_mask::QuestLogSlot],
) {
    use codec::update_mask::{full_quest_log_mask, idx, UpdateMaskValues};
    if !super::has_world_session(ctx, guid) {
        return;
    }
    let before = full_quest_log_mask(before);
    let after = full_quest_log_mask(after);
    let mut changes = UpdateMaskValues::new();
    for field in idx::PLAYER_QUEST_LOG_1_1..idx::PLAYER_QUEST_LOG_1_1 + 3 * idx::QUEST_LOG_SLOTS {
        if before.get(field) != after.get(field) {
            changes.set_u32(field, after.get(field).unwrap_or(0));
        }
    }
    if changes.block_count() != 0 {
        let (opcode, body) = codec::build_values_update_raw(guid, &changes);
        super::send_raw_to_character(ctx, guid, opcode, body);
    }
}

pub(crate) fn event_credit(ctx: &ReducerContext, row: &CharacterQuestEventCredit, inserted: bool) {
    if !super::has_world_session(ctx, row.character_guid) {
        return;
    }
    if inserted {
        super::send_to_character(
            ctx,
            row.character_guid,
            ServerOpcodeMessage::SMSG_QUESTUPDATE_COMPLETE(codec::build_questupdate_complete(
                row.quest_entry,
            )),
            super::EncodeFailure::Panic,
        );
    }
    refresh(ctx, row.character_guid);
}

pub(crate) fn group_event(
    ctx: &ReducerContext,
    recipient: u64,
    kind: u8,
    other: u64,
    payload: &str,
) {
    use lyracore_shared::quest::share_event_kind;
    if !super::has_world_session(ctx, recipient) {
        return;
    }
    let packets = match kind {
        share_event_kind::QUEST_SHARE => payload
            .parse()
            .map(|quest| details(ctx, other, quest))
            .unwrap_or_default(),
        share_event_kind::QUEST_PUSH_RESULT => payload
            .parse()
            .ok()
            .map(|code| {
                super::bounded(ServerOpcodeMessage::MSG_QUEST_PUSH_RESULT(Box::new(
                    codec::build_quest_push_result(other, code),
                )))
            })
            .into_iter()
            .collect(),
        _ => Vec::new(),
    };
    super::send_packets_to_character(ctx, recipient, packets);
}

pub(super) fn item_changed(ctx: &ReducerContext, guid: u64, entry: u32) {
    if collects(ctx, guid, entry) {
        refresh(ctx, guid);
    }
}

/// Whether an active quest of the character has an objective to collect the item.
pub(super) fn collects(ctx: &ReducerContext, guid: u64, entry: u32) -> bool {
    ctx.db
        .game_character_quest()
        .by_character_active()
        .filter((guid, false, false))
        .any(|quest| {
            ctx.db
                .game_quest_objective()
                .by_quest()
                .filter(quest.quest_entry)
                .any(|objective| {
                    objective.kind == objective_kind::COLLECT_ITEM
                        && objective.target_entry == entry
                })
        })
}
