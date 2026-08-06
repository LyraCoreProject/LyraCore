//! Data-driven creature EventAI (193) — FIRST SLICE: on-aggro SAY/YELL. A creature that has a
//! `game_creature_ai_event` row for its entry (event ON_AGGRO) speaks its line the instant it aggros a
//! player — a growl or a threat that gives 1-20 pulls life.
//!
//! Grafts onto machinery that already exists, ZERO core edits: the `on_aggro` notify hook
//! (`fire_on_aggro` in creatures/tick.rs, fired at the aggro-arm edge) drives it, and the yell ships via
//! the existing `game_chat_event` relay (a creature is a valid `sender_guid`) — no new wire plumbing.
//!
//! Fixture-first (like graveyards / rest triggers): a few Elwynn barks are seeded in `seed.rs` until the
//! mangos `creature_ai_scripts` importer bridge lands (the deferred larger slice — TIMED_IN_COMBAT casts,
//! HP%-flee, CALL_FOR_HELP, SUMMON, phases). The table is EventAI-shaped (`event_type`/`action_type`) so
//! those extend it as data + a handler arm, not a schema churn.

use spacetimedb::{table, ReducerContext, Table};

use crate::chat::{ChatEvent, CHAT_SAY, CHAT_YELL};
use crate::game_chat_event; // accessor trait
use crate::game_melee_attack; // accessor trait
use crate::game_world_entity; // accessor trait

/// EventAI event types (a subset of mangos `creature_ai_scripts.event_type`, OUR taxonomy).
/// HP%-below / on-death etc. remain deferred.
pub(crate) const AI_EVENT_ON_AGGRO: u8 = 0;
/// Fires on a repeating in-combat timer (mangos EVENT_T_TIMER_IN_COMBAT) — the bread and butter
/// of boss abilities (Rhahk'Zor Slam every 8-14s). Timing lives on the rule row; the per-creature
/// timer state lives in [`CreatureAiTimer`], owned wholesale by the `creature_ai_timed` pass.
pub(crate) const AI_EVENT_TIMED_IN_COMBAT: u8 = 1;
/// EventAI action types. SAY/YELL emit a `game_chat_event`; CAST casts `spell_id` at the
/// creature's current melee victim; EMOTE/FLEE/SUMMON are deferred.
pub(crate) const AI_ACTION_SAY: u8 = 0;
pub(crate) const AI_ACTION_YELL: u8 = 1;
pub(crate) const AI_ACTION_CAST: u8 = 2;

/// One creature EventAI rule: for `creature_entry`, when `event_type` fires, do `action_type`. For a
/// SAY/YELL action, `text` is the spoken line; for CAST, `spell_id` is the cast. MODULE-ONLY (no
/// `public`, no gateway binding) — barks leave via `game_chat_event` and casts via the normal spell
/// pipeline, both already relayed; nothing subscribes to this table. `by_entry` indexes the lookups.
/// The 193-CAST columns are END-appended + defaulted so the existing bark rows auto-migrate.
#[table(accessor = game_creature_ai_event, index(accessor = by_entry, btree(columns = [creature_entry])))]
pub struct CreatureAiEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub creature_entry: u32,
    pub event_type: u8,
    pub action_type: u8,
    pub text: String,
    /// The spell a CAST action fires (0 for SAY/YELL rows).
    #[default(0u32)]
    pub spell_id: u32,
    /// TIMED_IN_COMBAT initial-fire window after engaging (ms, random in [min, max]).
    #[default(0u32)]
    pub initial_min_ms: u32,
    #[default(0u32)]
    pub initial_max_ms: u32,
    /// TIMED_IN_COMBAT re-fire window after each fire (ms, random in [min, max]).
    #[default(0u32)]
    pub repeat_min_ms: u32,
    #[default(0u32)]
    pub repeat_max_ms: u32,
}

/// Per-creature armed timer for one TIMED_IN_COMBAT rule — the whole lifecycle (arm on engage,
/// fire+re-arm while fighting, reap on death/evade/despawn) is owned by the `creature_ai_timed`
/// tick pass, so no hook plumbing and no leak: a timer whose creature stopped fighting is deleted
/// the next tick. MODULE-ONLY. [entity]
#[table(accessor = game_creature_ai_timer, index(accessor = by_creature, btree(columns = [creature_guid])))]
pub struct CreatureAiTimer {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub creature_guid: u64,
    pub rule_id: u64,
    pub next_at_ms: u64,
}

/// Random ms in `[min, max]` (min when the span is empty/inverted — a fixed-cadence rule).
fn roll_ms(ctx: &ReducerContext, min: u32, max: u32) -> u64 {
    if max > min {
        (min + ctx.random::<u32>() % (max - min + 1)) as u64
    } else {
        min as u64
    }
}

// EventAI on-aggro (193): when a creature aggros a player DIRECTLY, speak any ON_AGGRO SAY/YELL rules for
// its entry as a `game_chat_event`. Registered on the existing `on_aggro` hook — no tick.rs edit. Assist
// (pack-answer) aggro is skipped so the puller's mob speaks and the whole pack doesn't shout in unison.
crate::game_hook!(on_aggro, fn creature_ai_on_aggro(ctx, payload) {
    let Some(creature) = ctx.db.game_world_entity().guid().find(payload.creature_guid) else {
        return;
    };
    let rules: Vec<CreatureAiEvent> = ctx
        .db
        .game_creature_ai_event()
        .by_entry()
        .filter(&creature.entry)
        .filter(|r| r.event_type == AI_EVENT_ON_AGGRO)
        .collect();
    let level = creature.level as u8;
    for r in rules {
        match r.action_type {
            // Only the directly-pulled creature SPEAKS (anti-spam for pack assist)…
            AI_ACTION_SAY | AI_ACTION_YELL if !payload.assist => {
                let chat_type = if r.action_type == AI_ACTION_YELL { CHAT_YELL } else { CHAT_SAY };
                ctx.db.game_chat_event().insert(ChatEvent {
                    id: 0,
                    sender_guid: payload.creature_guid,
                    chat_type,
                    language: 0, // Universal — monster speech every player reads
                    message: r.text,
                    created_at: ctx.timestamp,
                });
            }
            // …but an assist-aggroed caster still OPENS with its cast (193-CAST): the whole pack
            // firebolting the puller is exactly the vanilla pull experience. Err swallowed —
            // range/cooldown rejects just mean no opener this pull.
            AI_ACTION_CAST if r.spell_id != 0 => {
                let _ = crate::spell::begin_cast(
                    ctx,
                    payload.creature_guid,
                    r.spell_id,
                    level,
                    payload.target_guid,
                    false,
                    None,
                );
            }
            _ => {}
        }
    }
});

// TIMED_IN_COMBAT CAST engine (193-CAST) — one pass owns the whole timer lifecycle, so it works
// no matter HOW the fight started (proximity aggro, retaliation, a bot's pull; no hook needed):
//   ARM  — an engaged creature (melee-attacker row) whose entry has TIMED rules but no timer rows
//          gets one per rule at now + [initial_min, initial_max].
//   FIRE — a due timer casts the rule's spell at the creature's current melee victim via the SAME
//          `begin_cast` the creature rotation pass uses (GCD/cooldown/range arbitrated there),
//          then re-arms at [repeat_min, repeat_max] — or a short 1.5s retry on a reject, so a
//          boss chasing back into range doesn't skip a whole cycle.
//   REAP — a timer whose creature is gone, dead, or no longer fighting is deleted.
crate::game_tick_pass!(fn creature_ai_timed(ctx) {
    let now = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u64;
    let timers = ctx.db.game_creature_ai_timer();
    let entities = ctx.db.game_world_entity();
    let melee = ctx.db.game_melee_attack();
    let rules = ctx.db.game_creature_ai_event();

    // FIRE + REAP over the armed timers.
    for t in timers.iter().collect::<Vec<_>>() {
        let alive_and_fighting = entities
            .guid()
            .find(t.creature_guid)
            .map(|c| !c.dead)
            .unwrap_or(false)
            && melee.attacker_guid().find(t.creature_guid).is_some();
        if !alive_and_fighting {
            timers.id().delete(t.id);
            continue;
        }
        if now < t.next_at_ms {
            continue;
        }
        let Some(rule) = rules.id().find(t.rule_id) else {
            timers.id().delete(t.id); // rule edited away live — timer follows
            continue;
        };
        let target = melee.attacker_guid().find(t.creature_guid).map(|r| r.target_guid);
        let (Some(target), Some(c)) = (target, entities.guid().find(t.creature_guid)) else { continue };
        let ok = rule.spell_id != 0
            && crate::spell::begin_cast(ctx, t.creature_guid, rule.spell_id, c.level as u8, target, false, None)
                .is_ok();
        let mut t2 = t;
        t2.next_at_ms = now
            + if ok { roll_ms(ctx, rule.repeat_min_ms, rule.repeat_max_ms) } else { 1_500 };
        timers.id().update(t2);
    }

    // ARM engaged creatures that have TIMED rules but no timers yet (small tables both).
    for row in melee.iter() {
        let Some(c) = entities.guid().find(row.attacker_guid) else { continue };
        if c.is_player() || c.dead {
            continue;
        }
        for rule in rules
            .by_entry()
            .filter(&c.entry)
            .filter(|r| r.event_type == AI_EVENT_TIMED_IN_COMBAT && r.action_type == AI_ACTION_CAST)
        {
            let armed = timers
                .by_creature()
                .filter(&c.guid)
                .any(|t| t.rule_id == rule.id);
            if !armed {
                let initial = roll_ms(ctx, rule.initial_min_ms, rule.initial_max_ms);
                timers.insert(CreatureAiTimer {
                    id: 0,
                    creature_guid: c.guid,
                    rule_id: rule.id,
                    next_at_ms: now + initial,
                });
            }
        }
    }
});

/// Seed the fixture on-aggro barks (a few iconic Elwynn pulls) until the mangos `creature_ai_scripts`
/// importer bridge lands. Idempotent — clears the table first, so re-running (from `init` on a fresh DB
/// or the debug reducer on a live one) leaves exactly this set. Deliberate simplification: a flat
/// fixture list, replaced wholesale by the importer's per-entry rows later.
pub(crate) fn seed_on_aggro_fixtures(ctx: &ReducerContext) {
    let t = ctx.db.game_creature_ai_event();
    for r in t.iter().collect::<Vec<_>>() {
        t.id().delete(r.id);
    }
    // (creature_entry, action, line). Real Elwynn entries so a live pull actually speaks.
    let barks: &[(u32, u8, &str)] = &[
        (6, AI_ACTION_SAY, "Grrr!"),          // Kobold Vermin
        (116, AI_ACTION_YELL, "Intruder!"),   // Defias Bandit
        (448, AI_ACTION_YELL, "Roar!"),       // Hogger (Elwynn named elite)
    ];
    for &(entry, action, text) in barks {
        t.insert(CreatureAiEvent {
            id: 0,
            creature_entry: entry,
            event_type: AI_EVENT_ON_AGGRO,
            action_type: action,
            text: text.to_string(),
            spell_id: 0,
            initial_min_ms: 0,
            initial_max_ms: 0,
            repeat_min_ms: 0,
            repeat_max_ms: 0,
        });
    }
    // TIMED_IN_COMBAT boss casts (193-CAST): the Deadmines signature abilities the choreography
    // package doesn't cover (it does doors/adds/phases; these are the repeating combat casts).
    // (entry, spell, initial min..max ms, repeat min..max ms) — ids verified against the imported
    // game_spell (Rhahk'Zor Slam 6304, shredder Distracting Pain 3603 / Terrify 7399, Gilnid
    // Molten Metal 5213 / Melt Ore 5159, Mr. Smite Smite Stomp 6432); cadences are the cmangos
    // creature_ai_scripts ballparks [V].
    let boss_casts: &[(u32, u32, u32, u32, u32, u32)] = &[
        (644, 6304, 5_000, 9_000, 8_000, 14_000), // Rhahk'Zor — Rhahk'Zor Slam
        (642, 3603, 4_000, 8_000, 9_000, 14_000), // Sneed's Shredder — Distracting Pain
        (642, 7399, 10_000, 15_000, 18_000, 25_000), // Sneed's Shredder — Terrify
        (1763, 5213, 4_000, 7_000, 9_000, 13_000), // Gilnid — Molten Metal
        (1763, 5159, 8_000, 12_000, 14_000, 18_000), // Gilnid — Melt Ore
        (646, 6432, 6_000, 10_000, 12_000, 17_000), // Mr. Smite — Smite Stomp
    ];
    for &(entry, spell, imin, imax, rmin, rmax) in boss_casts {
        t.insert(CreatureAiEvent {
            id: 0,
            creature_entry: entry,
            event_type: AI_EVENT_TIMED_IN_COMBAT,
            action_type: AI_ACTION_CAST,
            text: String::new(),
            spell_id: spell,
            initial_min_ms: imin,
            initial_max_ms: imax,
            repeat_min_ms: rmin,
            repeat_max_ms: rmax,
        });
    }
}

/// Headless/live apply of the fixture barks (the live DB skips `init`'s one-time seed).
#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
pub fn debug_seed_creature_ai_fixtures(ctx: &ReducerContext) {
    seed_on_aggro_fixtures(ctx);
}
