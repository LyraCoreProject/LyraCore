use crate::actor::ActionRefusal;
use crate::quest::*;
use crate::{game_creature_template, game_item_template};
use lyracore_shared::quest::AcceptanceRefusal;
use spacetimedb::ReducerContext;

pub(super) fn quest_detail_view(
    ctx: &ReducerContext,
    quest_id: u32,
) -> Option<lyracore_protocol::codec::QuestDetailView> {
    use lyracore_protocol::codec::QuestDetailView;
    let tmpl = ctx.db.game_quest_template().entry().find(quest_id)?;
    let reward_xp = lyracore_shared::quest::base_reward_xp(
        tmpl.reward_xp,
        tmpl.reward_money_max_level,
        tmpl.quest_level,
    );
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
                0 => (o.target_entry, o.required_count, 0u32, 0u32),
                1 => (0, 0, o.target_entry, o.required_count),
                2 => (0u32.wrapping_sub(o.target_entry), o.required_count, 0, 0),
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
        money_reward: tmpl.reward_money,
        reward_xp,
        next_quest_id: tmpl.next_quest_id,
        max_level_money_reward: lyracore_shared::quest::max_level_money_reward(reward_xp),
        rewards: quest_rewards(ctx, quest_id),
        choice_rewards: quest_choice_rewards(ctx, quest_id),
        objectives: raw_objs
            .into_iter()
            .map(|(_, a, b, c, d)| (a, b, c, d))
            .collect(),
    })
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
                0 => {
                    let name = ctx
                        .db
                        .game_creature_template()
                        .entry()
                        .find(o.target_entry)
                        .map(|c| c.name.clone())
                        .unwrap_or_default();
                    format!("{name} slain: 0/{}", o.required_count)
                }
                1 => {
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
use super::{game_native_player, Rollback};
use lyracore_protocol::{
    codec,
    messages::vanilla::opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
    world::Packet,
};
use spacetimedb::ConnectionId;

fn packet(message: ServerOpcodeMessage) -> Result<Packet, String> {
    lyracore_protocol::world::encode(message).map_err(|error| error.to_string())
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

fn giver_relations(ctx: &ReducerContext, giver: u64) -> Vec<(u32, u8)> {
    use crate::{game_gameobject, game_world_entity};
    if let Some(entity) = ctx.db.game_world_entity().guid().find(giver) {
        ctx.db
            .game_creature_quest()
            .by_creature()
            .filter(entity.entry)
            .map(|r| (r.quest_entry, r.role))
            .collect()
    } else if let Some(object) = ctx.db.game_gameobject().guid().find(giver) {
        ctx.db
            .game_gameobject_quest()
            .by_gameobject()
            .filter(object.template_entry)
            .map(|r| (r.quest_entry, r.role))
            .collect()
    } else {
        Vec::new()
    }
}

fn next_in_chain(ctx: &ReducerContext, giver: u64, quest: u32) -> Option<u32> {
    let next = ctx
        .db
        .game_quest_template()
        .entry()
        .find(quest)?
        .next_quest_id;
    giver_relations(ctx, giver)
        .contains(&(next, codec::ROLE_START))
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
        .filter_map(|(entry, role)| {
            let template = ctx.db.game_quest_template().entry().find(entry)?;
            let row = character_quest_row(ctx, guid, entry);
            let active = row.as_ref().is_some_and(|row| !row.rewarded && !row.failed);
            Some(codec::GiverQuestEval {
                quest_id: entry,
                title: template.title.clone(),
                level: template.quest_level,
                role,
                startable: role == codec::ROLE_START
                    && accept_gates(ctx, &actor, &template).is_ok(),
                active,
                complete: active && row.as_ref().is_some_and(|row| quest_is_complete(ctx, row)),
            })
        })
        .collect()
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
        eval.quest_id == detail.quest_id && eval.role == codec::ROLE_END && eval.active
    });
    match turn_in {
        None => Ok(details(ctx, giver, detail.quest_id)),
        Some(eval) if eval.complete => Ok(vec![packet(
            ServerOpcodeMessage::SMSG_QUESTGIVER_OFFER_REWARD(Box::new(codec::build_offer_reward(
                giver, &detail,
            ))),
        )?]),
        Some(_) => Ok(vec![packet(
            ServerOpcodeMessage::SMSG_QUESTGIVER_REQUEST_ITEMS(Box::new(
                codec::build_request_items(giver, &detail, false),
            )),
        )?]),
    }
}

fn reward_screen(
    ctx: &ReducerContext,
    giver: u64,
    guid: u64,
    quest: u32,
) -> Result<Vec<Packet>, String> {
    let Some(detail) = quest_detail_view(ctx, quest) else {
        return Ok(Vec::new());
    };
    let complete = evaluations(ctx, giver, guid)
        .iter()
        .any(|eval| eval.quest_id == quest && eval.role == codec::ROLE_END && eval.complete);
    Ok(vec![packet(if complete {
        ServerOpcodeMessage::SMSG_QUESTGIVER_OFFER_REWARD(Box::new(codec::build_offer_reward(
            giver, &detail,
        )))
    } else {
        ServerOpcodeMessage::SMSG_QUESTGIVER_REQUEST_ITEMS(Box::new(codec::build_request_items(
            giver, &detail, false,
        )))
    })?])
}

fn accept_refusal(refusal: &AcceptQuestRefusal) -> Vec<Packet> {
    use lyracore_protocol::messages::vanilla::QuestFailedReason as Reason;
    let message = match refusal {
        AcceptQuestRefusal::LogFull => Some(ServerOpcodeMessage::SMSG_QUESTLOG_FULL),
        AcceptQuestRefusal::Acceptance { refusal, .. } => {
            let reason = match refusal {
                AcceptanceRefusal::Level => Reason::QuestFailedLowLevel,
                AcceptanceRefusal::Race => Reason::QuestFailedWrongRace,
                AcceptanceRefusal::AlreadyTaken => Reason::QuestAlreadyOn,
                AcceptanceRefusal::Class | AcceptanceRefusal::Prerequisite => Reason::DontHaveReq,
            };
            Some(ServerOpcodeMessage::SMSG_QUESTGIVER_QUEST_INVALID(
                codec::build_questgiver_quest_invalid(reason),
            ))
        }
        AcceptQuestRefusal::Refused(_) => None,
    };
    message
        .into_iter()
        .chain([ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE])
        .map(|message| packet(message).expect("fixed-size quest refusal fits a world frame"))
        .collect()
}

pub(super) fn receive(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    request: ClientOpcodeMessage,
) -> Result<(), Rollback> {
    let packets = match request {
        ClientOpcodeMessage::CMSG_QUESTGIVER_STATUS_QUERY(q) => vec![packet(
            ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(Box::new(codec::build_questgiver_status(
                q.guid.guid(),
                codec::quest_giver_status(&evaluations(ctx, q.guid.guid(), guid)),
            ))),
        )?],
        ClientOpcodeMessage::CMSG_QUESTGIVER_HELLO(q) => {
            use crate::game_world_entity;
            let actor = super::actor(ctx, guid)?;
            if ctx
                .db
                .game_world_entity()
                .guid()
                .find(q.guid.guid())
                .is_some_and(|npc| crate::reputation::npc_refuses_interaction(ctx, &npc, &actor))
            {
                return Ok(());
            }
            menu(ctx, q.guid.guid(), guid)?
        }
        ClientOpcodeMessage::CMSG_QUESTGIVER_QUERY_QUEST(q) => {
            details(ctx, q.guid.guid(), q.quest_id)
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
            Vec::new()
        }
        ClientOpcodeMessage::CMSG_QUESTGIVER_CANCEL => {
            vec![packet(ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE)?]
        }
        ClientOpcodeMessage::CMSG_QUESTGIVER_COMPLETE_QUEST(q) => {
            reward_screen(ctx, q.guid.guid(), guid, q.quest_id)?
        }
        ClientOpcodeMessage::CMSG_QUESTGIVER_REQUEST_REWARD(q) => {
            reward_screen(ctx, q.guid.guid(), guid, q.quest_id)?
        }
        ClientOpcodeMessage::CMSG_QUESTGIVER_CHOOSE_REWARD(q) => {
            if let Err(reason) = super::actor(ctx, guid).and_then(|_| {
                request_turn_in_quest(ctx, guid, q.guid.guid(), q.quest_id, q.reward)?;
                crate::mail_reward::file_reward_letter(ctx, guid, q.guid.guid(), q.quest_id)
            }) {
                let packets = quest_detail_view(ctx, q.quest_id)
                    .map(|detail| {
                        packet(ServerOpcodeMessage::SMSG_QUESTGIVER_OFFER_REWARD(Box::new(
                            codec::build_offer_reward(q.guid.guid(), &detail),
                        )))
                    })
                    .transpose()?
                    .into_iter()
                    .collect();
                return Err(Rollback::replies(packets, reason));
            }
            let mut packets: Vec<Packet> = quest_detail_view(ctx, q.quest_id)
                .map(|detail| {
                    packet(ServerOpcodeMessage::SMSG_QUESTGIVER_QUEST_COMPLETE(
                        Box::new(codec::build_quest_complete(&detail)),
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
        _ => return Err("invalid quest opcode".into()),
    };
    for packet in packets {
        super::send_raw(ctx, connection, packet.opcode, packet.body, false);
    }
    Ok(())
}

pub(super) fn refresh(ctx: &ReducerContext, guid: u64) {
    let Some(owner) = ctx.db.game_native_player().character_guid().find(guid) else {
        return;
    };
    let slots = super::view::quests(ctx, guid);
    let (opcode, body) =
        codec::build_values_update_raw(guid, &codec::update_mask::full_quest_log_mask(&slots));
    super::send_raw(ctx, owner.connection_id, opcode, body, false);
}

pub(crate) fn changed(
    ctx: &ReducerContext,
    old: Option<&CharacterQuest>,
    new: Option<&CharacterQuest>,
) {
    let Some(row) = new.or(old) else {
        return;
    };
    let Some(owner) = ctx
        .db
        .game_native_player()
        .character_guid()
        .find(row.character_guid)
    else {
        return;
    };
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
            messages.push(ServerOpcodeMessage::SMSG_QUESTUPDATE_FAILEDTIMER(
                codec::build_questupdate_failedtimer(row.quest_entry),
            ));
        }
        for message in messages {
            let packet = packet(message).expect("bounded quest progress fits a world frame");
            super::send_raw(ctx, owner.connection_id, packet.opcode, packet.body, false);
        }
    }
    refresh(ctx, row.character_guid);
}

pub(crate) fn event_credit(ctx: &ReducerContext, row: &CharacterQuestEventCredit, inserted: bool) {
    let Some(owner) = ctx
        .db
        .game_native_player()
        .character_guid()
        .find(row.character_guid)
    else {
        return;
    };
    if inserted {
        let packet = packet(ServerOpcodeMessage::SMSG_QUESTUPDATE_COMPLETE(
            codec::build_questupdate_complete(row.quest_entry),
        ))
        .expect("fixed-size quest completion fits a world frame");
        super::send_raw(ctx, owner.connection_id, packet.opcode, packet.body, false);
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
    let Some(owner) = ctx.db.game_native_player().character_guid().find(recipient) else {
        return;
    };
    let packets = match kind {
        share_event_kind::QUEST_SHARE => payload
            .parse()
            .map(|quest| details(ctx, other, quest))
            .unwrap_or_default(),
        share_event_kind::QUEST_PUSH_RESULT => payload
            .parse()
            .ok()
            .map(|code| {
                packet(ServerOpcodeMessage::MSG_QUEST_PUSH_RESULT(Box::new(
                    codec::build_quest_push_result(other, code),
                )))
                .expect("fixed-size quest share result fits a world frame")
            })
            .into_iter()
            .collect(),
        _ => Vec::new(),
    };
    for packet in packets {
        super::send_raw(ctx, owner.connection_id, packet.opcode, packet.body, false);
    }
}

pub(super) fn item_changed(ctx: &ReducerContext, guid: u64, entry: u32) {
    if ctx
        .db
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
    {
        refresh(ctx, guid);
    }
}
