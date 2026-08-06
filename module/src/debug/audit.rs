//! Content audits: the 1-20 class-kit audit (which learnable abilities actually DO something) and
//! the 1-20 quest-chain integrity audit (which quests would stall a leveling playthrough). Both are
//! pure reads that log a report; neither mutates state.

use spacetimedb::{log, reducer, ReducerContext, Table};

use crate::{
    game_creature_template, game_item_template, game_quest_template, game_spell, game_spell_effect,
};

/// 1-20 leveling audit (2026-07-17): for each of the 6 human-class trainers, resolve EVERY offering
/// through the server's own `resolve_learn_target` (wrapper → rank) and classify the rank spell's
/// effects — LIVE (has a real handler), HOLLOW (all effects are unmapped E_SCRIPTED no-ops, script_id
/// 0 — the spell "exists" but does nothing), or NO-EFFECT (learn-only/passive, no effect rows). Logs a
/// per-class band summary (L1-9 / L10-19 / L20) + the concrete HOLLOW list (class · req_level · id ·
/// name) — the actionable "which class abilities don't work through 1-20" wall list. Read the module
/// log after calling. Pure read, no state change.
#[reducer]
pub fn debug_audit_class_kits(ctx: &ReducerContext) {
    use crate::game_trainer_spell;
    use crate::spell::{A_FLAG, E_SCRIPTED};
    // (class label, trainer creature entry) — the stable CLASS_TRAINERS map (playerbots mod.rs).
    const TRAINERS: &[(&str, u32)] = &[
        ("Warrior", 913),
        ("Paladin", 927),
        ("Rogue", 917),
        ("Priest", 377),
        ("Mage", 328),
        ("Warlock", 906),
    ];
    // PASSIVE combat skills whose mechanic lives in the COMBAT ENGINE, not as a spell effect
    // (verified 2026-07-17: combat/tables.rs parry/block in the attack table, combat/mod.rs off-hand
    // swings). Their spell has no live effect but the ability WORKS — so they are NOT hollow. Matched
    // by name (rank-suffix stripped by the resolver). Keeps the HOLLOW list actionable.
    const ENGINE_PASSIVES: &[&str] = &[
        "Parry",
        "Dodge",
        "Block",
        "Shield Block",
        "Dual Wield",
        "Defense",
        "Two-Handed Swords",
        "Detect Magic",
        "Slow Fall",
        "Unending Breath",
    ];
    // Classify a resolved rank spell: Some(true)=LIVE, Some(false)=HOLLOW (unmapped no-op), None=no
    // effects, or the passive-engine bucket via the returned name check at the call site.
    let classify = |rank: u32| -> Option<bool> {
        let effects: Vec<_> = ctx
            .db
            .game_spell_effect()
            .by_spell()
            .filter(&rank)
            .collect();
        if effects.is_empty() {
            return None;
        }
        // A_FLAG passive markers don't count as "does something"; an E_SCRIPTED with script_id 0 is
        // the unmapped no-op. Any other effect kind (or a scripted handler with a real id) is live.
        let live = effects
            .iter()
            .any(|e| e.kind != A_FLAG && !(e.kind == E_SCRIPTED && e.script_id == 0));
        Some(live)
    };
    let band = |lvl: u8| -> usize {
        if lvl >= 20 {
            2
        } else if lvl >= 10 {
            1
        } else {
            0
        }
    };
    log::info!("=== CLASS-KIT AUDIT (1-20 leveling) — LIVE / HOLLOW / NO-EFFECT per band ===");
    for (label, trainer) in TRAINERS {
        // [band][0]=live [1]=hollow [2]=no-effect
        let mut counts = [[0u32; 3]; 3];
        let mut hollow: Vec<(u8, u32, String)> = Vec::new();
        for off in ctx
            .db
            .game_trainer_spell()
            .iter()
            .filter(|t| t.trainer_entry == *trainer)
        {
            if off.required_level > 20 {
                continue;
            }
            let rank = crate::trainer::resolve_learn_target(ctx, off.spell_id);
            let b = band(off.required_level);
            let name = ctx
                .db
                .game_spell()
                .spell_id()
                .find(rank)
                .map(|s| s.name)
                .unwrap_or_default();
            match classify(rank) {
                Some(true) => counts[b][0] += 1,
                // An engine-implemented passive counts as LIVE (the mechanic works), not hollow.
                _ if ENGINE_PASSIVES.contains(&name.as_str()) => counts[b][0] += 1,
                Some(false) => {
                    counts[b][1] += 1;
                    hollow.push((off.required_level, rank, name));
                }
                None => counts[b][2] += 1,
            }
        }
        log::info!(
            "{label:8}  L1-9: {}L/{}H/{}N   L10-19: {}L/{}H/{}N   L20: {}L/{}H/{}N",
            counts[0][0],
            counts[0][1],
            counts[0][2],
            counts[1][0],
            counts[1][1],
            counts[1][2],
            counts[2][0],
            counts[2][1],
            counts[2][2],
        );
        hollow.sort_by_key(|(lvl, _, _)| *lvl);
        for (lvl, id, name) in hollow {
            log::info!("    HOLLOW {label} L{lvl:<2} {id:>6} {name}");
        }
    }
    log::info!("=== END CLASS-KIT AUDIT ===");
}

/// 1-20 quest-chain integrity audit (2026-07-17): for every quest with `quest_level` in 1..=20,
/// check the four things a leveling player needs to actually COMPLETE it, and log the broken ones —
/// the "which quests would stall a 1-20 playthrough" wall list. Checks: (1) a START giver exists (a
/// creature or GO offers it — else the quest is unreachable), (2) an END giver exists (else it can't
/// be turned in), (3) `prev_quest_id`, if set, resolves to a real quest (else a dangling chain), (4)
/// every KILL/COLLECT objective target + every reward item references an existing template. Pure read.
#[reducer]
pub fn debug_audit_quest_chains(ctx: &ReducerContext) {
    use crate::quest::quest_role;
    use crate::{
        game_creature_quest, game_gameobject_quest, game_quest_objective, game_quest_reward_item,
    };
    let cq = ctx.db.game_creature_quest();
    let goq = ctx.db.game_gameobject_quest();
    let has_giver = |quest: u32, role: u8| -> bool {
        cq.iter().any(|r| r.quest_entry == quest && r.role == role)
            || goq.iter().any(|r| r.quest_entry == quest && r.role == role)
    };
    let band = |lvl: u32| -> usize {
        if lvl >= 20 {
            2
        } else if lvl >= 10 {
            1
        } else {
            0
        }
    };
    let mut ok = [0u32; 3];
    let mut broken: Vec<(u32, u32, String, String)> = Vec::new(); // (level, entry, title, reasons)
    log::info!("=== QUEST-CHAIN AUDIT (1-20) — completability per band ===");
    for q in ctx.db.game_quest_template().iter() {
        if q.quest_level == 0 || q.quest_level > 20 {
            continue;
        }
        let mut reasons: Vec<&str> = Vec::new();
        if !has_giver(q.entry, quest_role::START) {
            reasons.push("no START giver");
        }
        if !has_giver(q.entry, quest_role::END) {
            reasons.push("no END giver");
        }
        if q.prev_quest_id != 0
            && ctx
                .db
                .game_quest_template()
                .entry()
                .find(q.prev_quest_id)
                .is_none()
        {
            reasons.push("dangling prev_quest");
        }
        for o in ctx.db.game_quest_objective().by_quest().filter(&q.entry) {
            use crate::quest::objective_kind;
            let missing = match o.kind {
                objective_kind::KILL_CREATURE => ctx
                    .db
                    .game_creature_template()
                    .entry()
                    .find(o.target_entry)
                    .is_none(),
                objective_kind::COLLECT_ITEM => ctx
                    .db
                    .game_item_template()
                    .entry()
                    .find(o.target_entry)
                    .is_none(),
                _ => false, // USE_GAMEOBJECT / EXPLORE: target validated elsewhere
            };
            if missing {
                reasons.push("bad objective target");
                break;
            }
        }
        for r in ctx.db.game_quest_reward_item().by_quest().filter(&q.entry) {
            if ctx
                .db
                .game_item_template()
                .entry()
                .find(r.item_entry)
                .is_none()
            {
                reasons.push("bad reward item");
                break;
            }
        }
        if reasons.is_empty() {
            ok[band(q.quest_level)] += 1;
        } else {
            broken.push((q.quest_level, q.entry, q.title.clone(), reasons.join(", ")));
        }
    }
    log::info!(
        "OK quests — L1-9: {}   L10-19: {}   L20: {}   (broken: {})",
        ok[0],
        ok[1],
        ok[2],
        broken.len()
    );
    broken.sort_by_key(|(lvl, _, _, _)| *lvl);
    for (lvl, entry, title, reasons) in broken {
        log::info!("    BROKEN L{lvl:<2} q{entry:<5} [{reasons}] {title}");
    }
    log::info!("=== END QUEST-CHAIN AUDIT ===");
}
