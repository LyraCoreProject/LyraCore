//! Quest-log / quest-giver cache-accessor methods — pure code-motion split of the former
//! `reads.rs`.

use anyhow::Result;
use spacetimedb_sdk::Table;

use super::super::bindings::*;
use super::super::connection::Coordinator;
use super::player_item_count;

impl Coordinator {
    /// Evaluate every quest a creature OR gameobject `giver_guid` starts/ends against `player_guid`,
    /// for the overhead status icon (`SMSG_QUESTGIVER_STATUS`) + the quest menu
    /// (`SMSG_QUESTGIVER_QUEST_LIST`). The giver is resolved EXACTLY like the module's
    /// `quest::validate_giver` fallback: a live `game_world_entity` first (a
    /// creature), else a spawned `game_gameobject` (GO 68 "Wanted Poster" starts q176 with NO creature
    /// giver at all; GO 55/56 "Lost Guards" corpses drive the q37/q45/q71 END chain) — never both, and
    /// never a live player (this reader doesn't special-case party-share givers; a
    /// player guid here matches the entity lookup (players share the table) but resolves to Giver::Creature(0) — a player entry is always 0, and no game_creature_quest row keys entry 0, so the result is empty, same as before). For each relation
    /// on the giver's entry (`game_creature_quest` or `game_gameobject_quest`): join its
    /// `game_quest_template`, look the quest up in the player's `game_character_quest` log (the
    /// coordinator bypasses RLS so it reads any player's log), and compute `startable` (a fresh,
    /// level-qualifying START), `active` (held + un-rewarded END), and `complete` (active + every
    /// objective's count met). One guard, all inline — no nested cache locks. The codec
    /// ([`crate::codec::quest_giver_status`]) folds these.
    pub fn quest_giver_evals(
        &self,
        giver_guid: u64,
        player_guid: u64,
    ) -> Result<Vec<crate::codec::GiverQuestEval>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        enum Giver {
            Creature(u32),
            GameObject(u32),
        }
        let giver = if let Some(e) = db.game_world_entity().guid().find(&giver_guid) {
            Giver::Creature(e.entry)
        } else if let Some(go) = db.game_gameobject().guid().find(&giver_guid) {
            Giver::GameObject(go.template_entry)
        } else {
            return Ok(Vec::new());
        };
        // No live player → no status (don't fabricate a level-0 phantom). A DEAD player (ghost) gets no
        // status either: the module rejects both accept AND turn-in while dead, so showing a `!`/`?` the
        // reducer would then reject is exactly the icon-vs-reducer mismatch we forbid.
        let Some(player) = db.game_world_entity().guid().find(&player_guid) else {
            return Ok(Vec::new());
        };
        if player.dead {
            return Ok(Vec::new());
        }
        let player_level = player.level;
        // Race/class packed in unit_bytes_0 (race | class<<8 | …), for the gates.
        let race = (player.unit_bytes_0 & 0xFF) as u8;
        let class = ((player.unit_bytes_0 >> 8) & 0xFF) as u8;
        // The player's quest log, guid-keyed (privileged read — RLS bypassed).
        let log: Vec<_> = db
            .game_character_quest()
            .iter()
            .filter(|q| q.character_guid == player_guid)
            .collect();
        let evals = match giver {
            Giver::Creature(entry) => db
                .game_creature_quest()
                .iter()
                .filter(|r| r.creature_entry == entry)
                .filter_map(|rel| {
                    eval_relation(
                        db,
                        rel.quest_entry,
                        rel.role,
                        &log,
                        player_guid,
                        player_level,
                        race,
                        class,
                    )
                })
                .collect(),
            Giver::GameObject(entry) => db
                .game_gameobject_quest()
                .iter()
                .filter(|r| r.go_entry == entry)
                .filter_map(|rel| {
                    eval_relation(
                        db,
                        rel.quest_entry,
                        rel.role,
                        &log,
                        player_guid,
                        player_level,
                        race,
                        class,
                    )
                })
                .collect(),
        };
        Ok(evals)
    }

    /// Build the detail view for one quest (the accept screen, the offer-reward screen, and the
    /// completion popup all read this): title, money, the RESOLVED XP reward (explicit, else the shared
    /// `lyracore_shared::quest::xp_reward` so it matches the module's grant), reward items joined with their
    /// display ids, and a synthesized objectives line per kill objective ("Creature slain: 0/N" from the
    /// creature-template name — quest body text isn't imported yet). `None` if the quest isn't loaded.
    pub fn quest_detail(&self, quest_id: u32) -> Result<Option<crate::codec::QuestDetailView>> {
        let guard = self.0.coord();
        Ok(quest_detail_view(&guard.conn.db, quest_id))
    }

    /// The player's active (un-rewarded) quests as quest-log descriptor slots (Phase 2: the L window).
    /// Deterministic slot assignment (sorted by quest_entry → slot 0..), capped at the 20 vanilla slots;
    /// each slot carries the quest id, per-objective counts, and a state byte (1 = all objectives met,
    /// else 0). The gateway encodes these into the `PLAYER_QUEST_LOG_*` fields. RLS-bypassed read.
    pub fn player_quest_log(
        &self,
        player_guid: u64,
    ) -> Result<Vec<crate::codec::update_mask::QuestLogSlot>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let quests: Vec<_> = db.game_character_quest().iter().collect();
        Ok(build_quest_log_slots(db, &quests, player_guid))
    }
}

/// Is every objective of `quest_entry` met for this player? MIRRORS the module's `quest_is_complete`
/// (module/src/quest.rs): a COLLECT_ITEM objective reads LIVE INVENTORY (its progress count is never bumped —
/// completion follows the bag), every other kind reads the per-objective progress count. THE FIX for
/// collect-quest turn-in: the gateway used to read `counts` ONLY, so the 121-of-174 collect objectives always
/// showed incomplete → the client's "Complete Quest" button stayed disabled and the zone's quest backbone was
/// un-turn-in-able. A quest with no objectives is trivially complete (a pure talk-to-giver quest).
fn quest_objectives_complete(
    db: &RemoteTables,
    quest_entry: u32,
    logrow: Option<&CharacterQuest>,
    owner_guid: u64,
) -> bool {
    const COLLECT_ITEM: u8 = 1; // == module objective_kind::COLLECT_ITEM
    db.game_quest_objective()
        .iter()
        .filter(|o| o.quest_entry == quest_entry)
        .all(|o| {
            let have = if o.kind == COLLECT_ITEM {
                player_item_count(db, owner_guid, o.target_entry)
            } else {
                logrow
                    .and_then(|q| q.counts.get(o.obj_index as usize).copied())
                    .unwrap_or(0)
            };
            have >= o.required_count
        })
}

/// Build one quest's `QuestDetailView` off `db` (any connection's cache carrying the public quest
/// tables — the privileged coordinator cache via `Coordinator::quest_detail`, OR a per-player
/// subscription's own cache when it's subscribed to them too). `None` if the quest isn't loaded.
/// Extracted from `Coordinator::quest_detail` so the QUEST_SHARE group-event relay
/// (`subscriptions.rs`) can build the SAME view over a cloned `Coordinator` handle without
/// duplicating this join logic — one chokepoint, not two copies to keep in sync.
pub(crate) fn quest_detail_view(
    db: &RemoteTables,
    quest_id: u32,
) -> Option<crate::codec::QuestDetailView> {
    use crate::codec::QuestDetailView;
    let tmpl = db.game_quest_template().entry().find(&quest_id)?;
    // MUST mirror module/src/quest.rs's award resolution exactly (explicit override → authentic
    // RewMoneyMaxLevel/0.6 → xp_reward placeholder) so the completion popup equals the grant. No
    // over-level penalty on either side yet (deferred — add to both together with the player's level).
    let reward_xp = if tmpl.reward_xp > 0 {
        tmpl.reward_xp
    } else if tmpl.reward_money_max_level > 0 {
        lyracore_shared::quest::quest_xp(tmpl.reward_money_max_level)
    } else {
        lyracore_shared::quest::xp_reward(tmpl.quest_level)
    };
    // Body text from the side table (cmangos Details/Objectives/OfferReward/RequestItems).
    let text = db.game_quest_text().quest_entry().find(&quest_id);
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
    // All objectives in obj_index order → wire (creature_or_go, count, req_item, req_item_count):
    //   KILL_CREATURE(0): (+creature_entry, count, 0, 0)
    //   COLLECT_ITEM(1):  (0, 0, item_entry, count)
    //   USE_GAMEOBJECT(2): (negative go_entry as u32, count, 0, 0) — client checks sign to tell apart
    //   EXPLORE_AREATRIGGER(3): no wire slot (area triggers have no entry in the objectives array)
    let mut raw_objs: Vec<(u8, u32, u32, u32, u32)> = db
        .game_quest_objective()
        .iter()
        .filter(|o| o.quest_entry == quest_id)
        .filter_map(|o| {
            let slot = match o.kind {
                0 => (o.target_entry, o.required_count, 0u32, 0u32),
                1 => (0, 0, o.target_entry, o.required_count),
                2 => ((-(o.target_entry as i32)) as u32, o.required_count, 0, 0),
                _ => return None, // EXPLORE_AREATRIGGER and unknown: no wire slot
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
        objectives_text: synthesized_objectives(db, quest_id, imported_objectives),
        offer_reward_text,
        request_items_text,
        money_reward: tmpl.reward_money,
        reward_xp,
        // Threaded from the template — chains (successor auto-offer) + the level-cap
        // payout preview (always computed; the client only surfaces it when the VIEWER is capped).
        next_quest_id: tmpl.next_quest_id,
        max_level_money_reward: lyracore_shared::quest::max_level_money_reward(reward_xp),
        rewards: quest_rewards(db, quest_id),
        choice_rewards: quest_choice_rewards(db, quest_id),
        objectives: raw_objs
            .into_iter()
            .map(|(_, a, b, c, d)| (a, b, c, d))
            .collect(),
    })
}

// One argument per column of the condition row being evaluated; the SDK row type is not in scope on this side of the read.
#[allow(clippy::too_many_arguments)]
/// Evaluate ONE giver relation (a `game_creature_quest` OR `game_gameobject_quest` row, already
/// filtered to the giver's entry by the caller) against `player_guid`'s quest log. Takes the bare
/// `quest_entry`/`role` rather than a table row type — `game_creature_quest` and `game_gameobject_quest`
/// have the identical `(quest_entry, role)` shape, so `quest_giver_evals` calls this
/// same function for both, keyed only on which relation table it iterated.
///
/// PARITY: `startable` answers the same question as the Module's `quest::accept_gates`
/// (`module/src/quest.rs`) — level, race, class, prerequisite chain, not already held — and the two
/// must agree, else the `!` icon offers a quest the accept reducer refuses. It stays a second copy
/// because it reads the Gateway's own subscription-cache row types and cannot call Module code; the
/// mask functions in `lyracore_shared::quest` are the part both tiers do share. A Gate changed in
/// `accept_gates` has to be changed here as well.
fn eval_relation(
    db: &RemoteTables,
    quest_entry: u32,
    role: u8,
    log: &[CharacterQuest],
    player_guid: u64,
    player_level: u32,
    race: u8,
    class: u8,
) -> Option<crate::codec::GiverQuestEval> {
    use crate::codec::{GiverQuestEval, ROLE_START};
    let tmpl = db.game_quest_template().entry().find(&quest_entry)?;
    let logrow = log.iter().find(|q| q.quest_entry == quest_entry);
    let active = logrow.map(|q| !q.rewarded).unwrap_or(false);
    // Complete = active AND every objective met — INVENTORY-AWARE for COLLECT_ITEM (see the helper).
    let complete = active && quest_objectives_complete(db, quest_entry, logrow, player_guid);
    // A repeatable quest already rewarded keeps its CharacterQuest row (module/src/quest.rs no longer
    // deletes it — deleting would erase the only record it was ever completed, breaking any OTHER quest's
    // `prev_quest_id` check below), so "no row" is not the right re-offer test for it: a rewarded row on a
    // repeatable quest is startable again, mirroring apply_accept_quest's duplicate-guard exception.
    let no_active_or_done =
        logrow.is_none() || (tmpl.repeatable && logrow.is_some_and(|q| q.rewarded));
    let startable = role == ROLE_START
        && no_active_or_done
        && player_level >= tmpl.min_level
        && lyracore_shared::quest::race_allowed(tmpl.required_races, race)
        && lyracore_shared::quest::class_allowed(tmpl.required_classes, class)
        && (tmpl.prev_quest_id == 0
            || log
                .iter()
                .any(|q| q.quest_entry == tmpl.prev_quest_id && q.rewarded));
    Some(GiverQuestEval {
        quest_id: tmpl.entry,
        title: tmpl.title.clone(),
        level: tmpl.quest_level,
        role,
        startable,
        active,
        complete,
    })
}

/// Build the player's quest-log descriptor slots from raw quest + objective rows. Shared by the login
/// read ([`Coordinator::player_quest_log`], over the privileged coordinator cache) and the live relay
/// (`subscriptions.rs`, over the player connection's own RLS-scoped cache) so both produce identical
/// slot assignments. Active (un-rewarded) quests for `player_guid`, sorted by quest_entry → slot 0..,
/// capped at the 20 vanilla slots; `state` is 1 when every objective is met (INVENTORY-AWARE for
/// COLLECT_ITEM, via `quest_objectives_complete` — so the log shows "(Complete)" for a collect quest),
/// else 0. Reads `db` (game_quest_objective + game_item_instance); both callers' caches see this player.
pub(crate) fn build_quest_log_slots(
    db: &RemoteTables,
    quests: &[CharacterQuest],
    player_guid: u64,
) -> Vec<crate::codec::update_mask::QuestLogSlot> {
    let mut active: Vec<&CharacterQuest> = quests
        .iter()
        .filter(|q| q.character_guid == player_guid && !q.rewarded)
        .collect();
    active.sort_by_key(|q| q.quest_entry); // deterministic slot order
    active
        .into_iter()
        .take(crate::codec::update_mask::idx::QUEST_LOG_SLOTS as usize)
        .enumerate()
        .map(|(i, q)| {
            let complete = quest_objectives_complete(db, q.quest_entry, Some(q), player_guid);
            crate::codec::update_mask::QuestLogSlot {
                slot: i as u8,
                quest_id: q.quest_entry,
                counts: q.counts.clone(),
                state: u8::from(complete), // 1 = complete, 0 = incomplete
                timer: 0,
            }
        })
        .collect()
}

/// A quest's guaranteed reward items joined with each item's `display_id` (0 if the template isn't loaded).
fn quest_rewards(db: &RemoteTables, quest_id: u32) -> Vec<crate::codec::QuestRewardView> {
    db.game_quest_reward_item()
        .iter()
        .filter(|r| r.quest_entry == quest_id)
        .map(|r| {
            let display_id = db
                .game_item_template()
                .entry()
                .find(&r.item_entry)
                .map(|t| t.display_id)
                .unwrap_or(0);
            crate::codec::QuestRewardView {
                item_entry: r.item_entry,
                count: r.count,
                display_id,
            }
        })
        .collect()
}

/// A quest's CHOICE reward items (pick-1-of-N), joined to each item's `display_id` (0 if the template
/// isn't loaded) and ORDERED BY `choice_index` so the wire position matches the index the client sends
/// back as `CMSG_QUESTGIVER_CHOOSE_REWARD.reward`.
fn quest_choice_rewards(db: &RemoteTables, quest_id: u32) -> Vec<crate::codec::QuestRewardView> {
    let mut rows: Vec<_> = db
        .game_quest_reward_choice()
        .iter()
        .filter(|r| r.quest_entry == quest_id)
        .collect();
    rows.sort_by_key(|r| r.choice_index);
    rows.into_iter()
        .map(|r| {
            let display_id = db
                .game_item_template()
                .entry()
                .find(&r.item_entry)
                .map(|t| t.display_id)
                .unwrap_or(0);
            crate::codec::QuestRewardView {
                item_entry: r.item_entry,
                count: r.count,
                display_id,
            }
        })
        .collect()
}

/// The objectives text for the detail window: the imported cmangos `Objectives` if present, else a
/// synthesized line per objective (kind-aware: kill = "Name slain: 0/N", collect = "Name: 0/N").
fn synthesized_objectives(db: &RemoteTables, quest_id: u32, imported: String) -> String {
    if !imported.trim().is_empty() {
        return imported;
    }
    let mut objs: Vec<(u8, String)> = db
        .game_quest_objective()
        .iter()
        .filter(|o| o.quest_entry == quest_id)
        .map(|o| {
            let text = match o.kind {
                0 => {
                    // KILL_CREATURE
                    let name = db
                        .game_creature_template()
                        .entry()
                        .find(&o.target_entry)
                        .map(|c| c.name.clone())
                        .unwrap_or_default();
                    format!("{name} slain: 0/{}", o.required_count)
                }
                1 => {
                    // COLLECT_ITEM
                    let name = db
                        .game_item_template()
                        .entry()
                        .find(&o.target_entry)
                        .map(|t| t.name.clone())
                        .unwrap_or_default();
                    format!("{name}: 0/{}", o.required_count)
                }
                _ => String::new(), // USE_GAMEOBJECT / EXPLORE: cmangos Objectives text covers these
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
