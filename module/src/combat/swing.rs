//! The swing tick (#382 split of the former monolithic `combat/mod.rs`, on top of #370's shared damage
//! pipeline): `tick_melee`'s three passes (`leash_pass`/`aggro_pass`/`resolve_swing`), the positional
//! gate (`swing_blocked`), and the resolvers that actually roll + fire a hit (`fire_melee_swing`,
//! `resolve_offhand_swing`, `fire_ranged_shot`, and the scheduled `ranged_impact` reducer). Every
//! resolver here routes through `death`'s shared `fold_incoming_damage`/`apply_hit` pipeline — see
//! `damage_pipeline_drift_tests` below, which pins that wiring. `mod.rs` re-exports this module
//! (`pub use swing::*`) so every `crate::combat::<sym>` path resolves regardless of which submodule
//! actually defines it.

use spacetimedb::{reducer, ReducerContext, ScheduleAt, Table, TimeDuration};

use crate::{
    game_item_instance, game_item_template, game_spell, game_spell_cast_event, game_world_entity,
    SpellCastEvent, WorldEntity,
};

// Tables' pure formulas/consts and the sibling submodules' re-exports (`roll_swing`, `apply_hit`,
// `enter_combat`, ...) are all pulled in from `mod.rs` (`pub use tables::*` + `pub use
// folds::*`/`death::*`/`engage::*`) so every symbol resolves the same as before the split.
use super::*;

/// Swing tick (scheduled; scheduler-only). Two passes:
/// 1. **Aggro** — a creature attacked by a player retaliates: ensure a reciprocal creature→player
///    engagement exists (one target at a time). No creature AI beyond "fight whoever hit me".
/// 2. **Swings** — for each engagement in range whose swing timer elapsed, deal damage
///    (player→7, creature→2). A creature target dies at 0 HP (DESTROY + free attackers + arm
///    respawn); a **player** target also dies at 0 HP (health=0 + `dead`, then release→ghost).
///
/// A leash pass runs first: a creature whose pursuit deadline expired with the target away from the
/// remembered refresh position evades.
///
/// Work-item 229 (per-instance ticks): DELIBERATELY LEFT GLOBAL — one schedule row, no `instance_id`
/// scoping. Verified same-instance-by-construction: every pass here outer-loops `game_melee_attack`,
/// and a melee row's attacker/target pair shares one instance at ARM time on every arming path —
/// `apply_start_attack`/`apply_start_ranged_attack` reject a cross-instance target explicitly, this
/// file's `aggro_pass` retaliation mirrors an existing (same-instance) row, and `tick_creatures`'s
/// aggro/assist/pet passes pair within one instance (190 slice 1). So this tick's cost is O(active
/// engagements) — it scales with combat, NOT with instance count, and scoping it per instance would
/// divide an already-small table while adding a per-row entity fetch. (Known pre-existing edge, not
/// widened here: 224's `teleport_player` can move one side of a live pair cross-instance AFTER
/// arming; the leash pass then evades the creature on raw coordinate distance.) The tick_ms
/// smoothing knob is `tick_creatures`'s per-instance row; melee swing timing is already 100ms
/// globally.
#[reducer]
pub fn tick_melee(ctx: &ReducerContext, _schedule: MeleeSchedule) {
    if ctx.sender() != ctx.database_identity() {
        return;
    }
    // Three named passes, run in this order. Each pass re-derives its own table handles /
    // now-timestamp from `ctx` (SpacetimeDB handles are live views, so a re-fetch is behavior-identical).
    // There is NO cross-pass shared state: the leash `evaders`, the aggro `new_aggro`, and the swing
    // `attacks` Vecs are each created and consumed within one pass.
    leash_pass(ctx);
    aggro_pass(ctx);
    resolve_swing(ctx);
}

/// Pass 0 — leash. A creature evades once its pursuit deadline has expired AND the target has left
/// `LEASH_RADIUS_SQ` of the position remembered at the last damage exchange (`ai::should_evade`), or
/// unconditionally past the absolute backstop. Evading drops ALL its combat and heals it to full (the
/// on_update health-VALUES relay refills the bar); the return pass walks it home.
///
/// Distance alone does not end a fight: while damage keeps flowing the engagement is refreshed, so combat
/// continues however far it travels from the creature's camp.
///
/// A row whose clock has never been stamped (freshly armed, or auto-migrated from before the field
/// existed) is SEEDED here instead of judged — otherwise an engagement that never sees a damage exchange
/// would never time out, and a max-range Auto Shot pull would evade before its first shot landed.
fn leash_pass(ctx: &ReducerContext) {
    let melee = ctx.db.game_melee_attack();
    let entities = ctx.db.game_world_entity();
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u32;

    let mut evaders: Vec<u64> = Vec::new();
    let mut seeds: Vec<(u64, f32, f32)> = Vec::new();
    for atk in melee.iter() {
        let (Some(attacker), Some(target)) = (
            entities.guid().find(atk.attacker_guid),
            entities.guid().find(atk.target_guid),
        ) else {
            continue;
        };
        // Only a creature's OWN engagement leashes (attacker is the creature, target is the player).
        if attacker.is_player() || evaders.contains(&attacker.guid) {
            continue;
        }
        // The absolute backstop, on the raw creature→target distance: the only distance that still
        // evades, so nothing stays in combat across a zone. 3D, as the old target cap was.
        let (dx, dy, dz) = (
            target.x - attacker.x,
            target.y - attacker.y,
            target.z - attacker.z,
        );
        if crate::creatures::beyond_combat_backstop(dx * dx + dy * dy + dz * dz) {
            evaders.push(attacker.guid);
            continue;
        }
        if atk.pursuit_ends_ms == 0 {
            seeds.push((atk.attacker_guid, attacker.x, attacker.y));
            continue;
        }
        // The target measured against the REMEMBERED position, never the creature against its spawn.
        // 2D like the wander/return-home math (z varies on slopes).
        let (rx, ry) = (target.x - atk.leash_x, target.y - atk.leash_y);
        if crate::creatures::should_evade(now_ms, atk.pursuit_ends_ms, rx * rx + ry * ry) {
            evaders.push(atk.attacker_guid);
        }
    }
    for (creature, x, y) in seeds {
        if let Some(mut row) = melee.attacker_guid().find(creature) {
            row.pursuit_ends_ms = crate::creatures::pursuit_deadline_ms(now_ms);
            row.leash_x = x;
            row.leash_y = y;
            melee.attacker_guid().update(row);
        }
    }
    for creature in evaders {
        // Drop every engagement touching the creature (its attack + attacks on it).
        disengage(ctx, creature);
        // Evade-heal to full (the on_update relay refills the bar for observers).
        if let Some(mut c) = entities.guid().find(creature) {
            if c.health != c.max_health {
                c.health = c.max_health;
                entities.guid().update(c);
            }
        }
    }
}

/// Pass 1 — aggro. A creature with an incoming player attack and no engagement of its own retaliates
/// against that attacker (one target at a time). Collect first, then insert (don't mutate while iterating).
fn aggro_pass(ctx: &ReducerContext) {
    let melee = ctx.db.game_melee_attack();
    let entities = ctx.db.game_world_entity();

    let mut new_aggro: Vec<(u64, u64)> = Vec::new(); // (creature, player)
    for atk in melee.iter() {
        let (Some(attacker), Some(target)) = (
            entities.guid().find(atk.attacker_guid),
            entities.guid().find(atk.target_guid),
        ) else {
            continue;
        };
        // Retaliate only when the attacker is actually in MELEE RANGE (the same leeway gate the
        // swing tick uses): arming melee auto-attack from 20 yd out must NOT aggro the creature —
        // vanilla mobs engage on proximity-aggro range or a LANDED hit, never on a player merely
        // entering attack stance at distance (user-observed bug). The armed row is harmless until
        // the player closes: their first in-range swing lands the same tick this pass fires.
        // A RANGED auto-repeat (Auto Shot / Shoot) retaliates at any distance — but only once a
        // shot has actually FIRED (the swing tick's `enter_combat` stamped the target IN_COMBAT).
        // Arming alone is NOT a hit (097 rev.2): the first shot is seeded ~500ms out and can be
        // rejected/suppressed, and retaliating at arm time let the wolf aggro-then-instantly-evade
        // on a pull whose first shot never fired ("enters combat, leaves combat immediately").
        let is_ranged = atk.ranged_spell_id != 0;
        let dx = attacker.x - target.x;
        let dy = attacker.y - target.y;
        let dz = attacker.z - target.z;
        // MELEE retaliation additionally requires a swing to have FIRED (`last_swing_ms` is 0 at
        // arm/retarget and only the tail-of-loop stamp sets it, after the range/facing/LoS gates
        // let a swing through). Without this a player standing IN range with their BACK to a
        // neutral wolf aggro'd it just by toggling attack — the facing gate ate every swing, so
        // the wolf was retaliating against an attack that never happened (user find; the melee
        // twin of the ranged IN_COMBAT gate above). Costs one 100ms tick vs the old same-tick arm.
        if attacker.is_player()
            && !target.is_player()
            && !target.dead // a corpse doesn't retaliate
            && ((is_ranged
                && target.unit_flags & lyracore_shared::constants::unit_flags::IN_COMBAT != 0)
                || (atk.last_swing_ms != 0
                    && dx * dx + dy * dy + dz * dz <= MELEE_RANGE_LEEWAY_SQ))
            && melee.attacker_guid().find(target.guid).is_none()
            && !new_aggro.iter().any(|(c, _)| *c == target.guid)
        {
            new_aggro.push((target.guid, attacker.guid));
        }
    }
    for (creature, player) in new_aggro {
        melee.insert(MeleeAttack {
            attacker_guid: creature,
            target_guid: player,
            last_swing_ms: 0,   // swing back immediately
            ranged_spell_id: 0, // creatures retaliate in melee
            last_offhand_swing_ms: 0,
            rout_ends_ms: 0,
            pursuit_ends_ms: 0,
            leash_x: 0.0,
            leash_y: 0.0,
        });
    }
}

/// The OFF-HAND swing (dual wield): a second, independent attack rolled by
/// `resolve_swing` for a MELEE engagement whenever `attacker_guid` has a live off-hand weapon
/// (`equipped_offhand_weapon_damage` — a shield/holdable off-hand short-circuits at `None`, unchanged
/// swing count). Runs on its OWN clock — the off-hand weapon's own `delay_ms`, tracked in
/// `MeleeAttack::last_offhand_swing_ms` — so it fires at a different cadence than the main hand and is
/// NOT gated by the main-hand swing-timer `continue` in `resolve_swing`.
///
/// Its own function rather than a branch of `fire_melee_swing`'s locals: it re-fetches its OWN fresh
/// `target` row (the main-hand swing may go on to damage or even kill the SAME target this tick) and
/// persists its own `last_offhand_swing_ms` stamp directly — it does not rely on the main hand's
/// tail-of-loop stamp, which that path's various early returns (corpse/CC/lethal) may skip.
///
/// The DAMAGE half is not duplicated at all any more (#370): the same
/// `fold_incoming_damage` → event → [`apply_hit`] the main-hand swing and the ranged impact use, over
/// the off-hand's AP-scaled range REDUCED by `apply_offhand_penalty` (vanilla's 50% dual-wield
/// penalty). What stays off-hand-specific is only what vanilla makes MAIN-HAND-only: it does not arm
/// the Overpower/Revenge react windows, does not fire the seal / next-swing procs, and does not wear
/// durability (no separate off-hand durability model yet — a deliberate simplification, like the
/// main-hand's own DURABILITY_WEAR_CHANCE_PCT tuning). It also leaves the IN_COMBAT stamp to the main
/// hand's tail (the engagement is shared).
fn resolve_offhand_swing(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
    attacker: &WorldEntity,
    now_ms: u32,
) {
    // No off-hand weapon (empty, shield/holdable, or broken) → no second stream. The common case for
    // every creature (no equipment) and every player without a second one-hander — a no-op that leaves
    // `last_offhand_swing_ms` untouched, so this is byte-identical to before for them.
    let Some((dmin, dmax, delay)) = equipped_offhand_weapon_damage(ctx, attacker_guid) else {
        return;
    };
    let melee = ctx.db.game_melee_attack();
    let Some(atk) = melee.attacker_guid().find(attacker_guid) else {
        return; // engagement row already gone (freed by something earlier this tick)
    };
    if atk.last_offhand_swing_ms != 0 && now_ms.wrapping_sub(atk.last_offhand_swing_ms) < delay {
        return; // off-hand still on its own cooldown
    }
    let Some(target) = ctx.db.game_world_entity().guid().find(target_guid) else {
        return;
    };
    if attacker.dead || target.dead {
        return; // mirrors resolve_swing's corpse guard
    }
    // Warrior Disarm (A_DISARM): mirrors the main-hand read in `swing_range_ctx`, folded
    // into this range derivation too — a disarmed PLAYER drops to UNARMED on the main hand, so there is
    // no dual-wield stream left to swing (an unarmed fighter has no off-hand weapon either). Without
    // this the off-hand kept swinging at full value while Disarm stripped only the main hand. `false`
    // for any un-disarmed attacker, so this stays a no-op without an A_DISARM aura (baseline-safe).
    if crate::spell::is_disarmed(ctx, attacker_guid) {
        return;
    }

    // Effective AP identical to the main-hand fold (swing_range_ctx's player branch), scaled by the
    // OFF-HAND weapon's own delay, then halved by the vanilla dual-wield penalty.
    let class = attacker.class();
    let ap = melee_attack_power_for(
        class,
        effective_strength(ctx, attacker),
        effective_agility(ctx, attacker),
        attacker.level,
    ) + aura_attack_power_bonus(ctx, attacker.guid);
    let (raw_min, raw_max) = weapon_swing_range_ap(ap, dmin, dmax, delay);
    let range = apply_offhand_penalty(raw_min, raw_max);

    let (rolled, hit_info, blocked) = roll_swing_with_range(ctx, attacker, &target, range);

    crate::spell::break_stealth(ctx, attacker_guid);

    // The SHARED pipeline (#370): outgoing % → incoming % → absorb → godmode, then the wire event,
    // then the application (rage/skill/lethal fork/health/break-on-damage/threat). Identical to the
    // main-hand swing by construction — this is exactly the copy whose Disarm gate drifted (#361).
    let (dmg, _absorbed) = fold_incoming_damage(ctx, attacker_guid, target_guid, rolled);
    let lethal = is_lethal(target.health, dmg);

    ctx.db.game_combat_event().insert(CombatEvent {
        damage: dmg,
        hit_info,
        killing_blow: lethal,
        blocked_amount: blocked,
        // the off-hand stream is always melee, never a ranged auto-attack (ranged_spell_id/
        // ammo_display_id/impact_delay_ms/spell_swing stay at the baseline's 0/0/0/false).
        ..CombatEvent::signal_at(ctx, attacker, target_guid)
    });

    if apply_hit(
        ctx,
        attacker_guid,
        target_guid,
        dmg,
        Hit::weapon(HitSource::OffHand, hit_info == HIT_CRIT),
    )
    .combat_ended()
    {
        // The kill's `disengage` already freed this engagement row, so there is no off-hand clock
        // left to stamp (the stamp below would find nothing) — return like the main-hand path does.
        return;
    }

    // Persist the off-hand's own clock — re-fetch fresh (the engagement row may not have been touched
    // by anything above, but read-your-writes is cheap insurance and matches the file's established
    // "re-fetch a fresh mutable copy" pattern for a row written mid-function).
    if let Some(mut fresh) = melee.attacker_guid().find(attacker_guid) {
        fresh.last_offhand_swing_ms = now_ms;
        melee.attacker_guid().update(fresh);
    }
}

/// Pass 2 — swings. For each engagement (re-read, since aggro may have added rows), run the SHARED
/// ELIGIBILITY GATE and, if it passes, fire the swing.
///
/// This function is the gate and nothing else (#370): both parties still in the world and not
/// corpses, the attacker not crowd-controlled and not routing, the ranged weapon still equipped, the
/// vanilla movement rule for a ranged loop, [`swing_blocked`]'s positional checks, the off-hand's
/// independent second stream, and the swing timer. What a swing then DOES lives in one of two
/// resolvers — [`fire_melee_swing`] or [`fire_ranged_shot`] — because a melee swing and a ranged shot
/// share their gate and almost nothing else. They used to be one ~370-line body with a mid-loop
/// `continue` fork, four repetitions of the ranged teardown rule, and `ranged.is_none()` sprinkled
/// through every melee-only proc.
fn resolve_swing(ctx: &ReducerContext) {
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u32;
    let melee = ctx.db.game_melee_attack();
    let entities = ctx.db.game_world_entity();

    let attacks: Vec<MeleeAttack> = melee.iter().collect();
    for atk in attacks {
        // Drop the engagement if either party left the world (also covers rows freed earlier this
        // tick by another engagement's killing blow).
        let Some(attacker) = entities.guid().find(atk.attacker_guid) else {
            melee.attacker_guid().delete(atk.attacker_guid);
            continue;
        };
        let Some(mut target) = entities.guid().find(atk.target_guid) else {
            melee.attacker_guid().delete(atk.attacker_guid);
            continue;
        };

        // A corpse neither swings nor is a valid swing target. With a lingering corpse
        // (the entity persists until the decay pass), a stale snapshot row whose attacker OR target
        // was killed earlier in THIS tick survives the two existence guards above (the corpse row is
        // still present). Without this guard such a row would: let a dead unit keep swinging
        // (attacker.dead); re-enter the killing-blow branch on a 0-HP target (target.dead → double
        // XP, duplicate death event, reset decay timer); and reach the `update` at the loop tail on a
        // row already freed by this tick's killing-blow `disengage`, which PANICS on a missing PK and
        // rolls back the entire tick. `delete` is idempotent if the row was already freed.
        if attacker.dead || target.dead {
            melee.attacker_guid().delete(atk.attacker_guid);
            continue;
        }
        if melee.attacker_guid().find(atk.attacker_guid).is_none() {
            continue; // an earlier hit in this snapshot already tore the engagement down
        }
        if !crate::combat::may_harm(ctx, &attacker, &target) {
            melee.attacker_guid().delete(atk.attacker_guid);
            continue; // friendship was restored, including an ended Duel
        }

        // Crowd control: an ACTION-blocked attacker (stunned/polymorphed/feared) cannot swing. Gated HERE — after the existence/corpse
        // guards (so a stale row is still reaped) but BEFORE the range/swing-timer checks below and the
        // `last_swing_ms` write at the loop tail — so a stun does NOT even reset the swing timer (the
        // attacker resumes its prior cadence the instant the stun ends, the vanilla feel). The
        // engagement ROW is LEFT INTACT (we don't `delete` it): the unit is still in combat, just
        // locked, so it retaliates the moment the stun lifts. `continue` only skips THIS row's swing —
        // the outer `for mut atk in attacks` reads an owned snapshot Vec, so skipping a row never
        // corrupts the iteration (no table cursor is held across the body). A ROOTED attacker is NOT
        // gated here — root blocks movement, not attacks, so a rooted-but-in-range unit keeps swinging.
        // Baseline-safe: `is_action_blocked` is `false` for any unit without a stun/poly/fear aura, so an
        // un-CC'd swing reaches the identical range/timer/roll path it did before — byte-for-byte. A
        // POLYMORPHED or FEARED attacker is action-blocked too, so it can't swing either (a feared unit
        // routs without retaliating).
        if crate::spell::is_action_blocked(ctx, atk.attacker_guid) {
            continue;
        }

        // A creature inside its ROUT window is RUNNING, not fighting — skip its swing while it routs. Like
        // the CC gate above, the engagement ROW is LEFT INTACT: routing is a SHARED combat state (the flee
        // pass keeps both sides in combat). Once the window closes the same creature swings again at
        // whatever health it has left, and a low-HP humanoid whose rout never started (frozen, feared)
        // keeps swinging throughout — both follow from the one shared predicate.
        if !attacker.is_player() && crate::creatures::tick::creature_is_routing(ctx, &attacker) {
            continue;
        }

        // RANGED vs MELEE: a ranged auto-attack (Auto Shot 75 / wand Shoot 5019) reads the slot-17
        // weapon, fires on ITS delay over a longer range, and uses a reduced attack table; a melee
        // engagement (ranged_spell_id == 0) is byte-identical to before. The ranged weapon is fetched
        // ONCE (a swing needs its damage + delay); if it's gone (unequipped mid-fight) the engagement
        // ends gracefully rather than firing bare-handed.
        let ranged = if atk.ranged_spell_id != 0 {
            match equipped_ranged_weapon(ctx, atk.attacker_guid) {
                Some(w) => Some(w),
                None => {
                    melee.attacker_guid().delete(atk.attacker_guid);
                    continue;
                }
            }
        } else {
            None
        };

        // (097/vanilla) A RANGED engagement whose shot comes DUE against a hard blocker (out of range /
        // too close / no LoS / not facing) is TORN DOWN, not silently suppressed — the vanilla
        // auto-repeat rule is that a failed check on a DUE ranged shot interrupts
        // the loop, and the row's delete relays the server-initiated SMSG_CANCEL_AUTO_REPEAT that
        // drops the client's toggle. Gated on DUE so a transient blocker BETWEEN shots (the mob
        // circling behind you, an LoS flicker) never cancels — vanilla only checks when the shot is
        // ready. Melee rows keep the silent per-tick `continue` (a melee swing resumes by itself).
        let ranged_due =
            ranged.is_some_and(|(_, _, delay, _)| now_ms.wrapping_sub(atk.last_swing_ms) >= delay);

        // (097/vanilla) Realtime movement rule — vanilla applies this BEFORE any castability
        // check: a PLAYER who is actually TRANSLATING (MOVE_MASK_MOVING — turning in place does
        // not count) CANCELS a wand loop outright, and DEFERS an Auto Shot loop: the due shot
        // re-arms RANGED_INITIAL_SHOT_MS out, over and over while moving, so the first shot after
        // stopping lands ~0.5s later and never fires mid-run (user bug: "we can shoot while
        // running"). Placed before the hard-fail gates so a moving out-of-range player defers
        // rather than cancels (vanilla order). Creatures are exempt (server-driven movement).
        if let Some((_, _, delay, subclass)) = ranged {
            use crate::items::weapon_subclass as ws;
            if attacker.is_player() && attacker.movement_flags & MOVE_MASK_MOVING != 0 {
                if subclass == ws::WAND {
                    // Wand Shoot dies on movement (vmangos: Category 351 → InterruptSpell); the row
                    // delete relays the server-initiated SMSG_CANCEL_AUTO_REPEAT.
                    melee.attacker_guid().delete(atk.attacker_guid);
                } else if ranged_due {
                    if let Some(mut fresh) = melee.attacker_guid().find(atk.attacker_guid) {
                        fresh.last_swing_ms = now_ms
                            .wrapping_add(RANGED_INITIAL_SHOT_MS)
                            .wrapping_sub(delay)
                            .max(1);
                        melee.attacker_guid().update(fresh);
                    }
                }
                continue;
            }
        }

        // POSITIONAL eligibility — max range, ranged minimum range, line of sight, facing — all four
        // in ONE gate (#370). They used to be four separate `if ... { if ranged_due { delete } continue }`
        // blocks, i.e. four copies of the teardown rule ("a blocker on a DUE ranged shot INTERRUPTS the
        // auto-repeat loop; a melee blocker just waits for the next tick") — one per gate, each free to
        // drift. `swing_blocked` is side-effect-free and short-circuits in the same order, so this is
        // behaviour-identical; the teardown now exists once.
        let (dx, dy, dz) = (
            target.x - attacker.x,
            target.y - attacker.y,
            target.z - attacker.z,
        );
        let dist_sq = dx * dx + dy * dy + dz * dz;
        if swing_blocked(ctx, &attacker, &target, ranged.is_some(), dist_sq, now_ms) {
            if ranged_due {
                melee.attacker_guid().delete(atk.attacker_guid);
            }
            continue;
        }

        // Off-hand swing (dual wield): a MELEE engagement (never ranged — Auto Shot/wand
        // have no off-hand analog) whose attacker has a live off-hand WEAPON rolls a SECOND, independent
        // swing on the off-hand's OWN clock (`last_offhand_swing_ms`), gated on the SAME in-range check
        // just above but NOT on the main-hand's swing timer below — so the off-hand still fires on a tick
        // where the main hand is mid-cooldown. Self-contained: re-fetches its own fresh `target` (the
        // main-hand branch below may go on to kill/damage the SAME target this tick) and writes its own
        // event + damage + rage/threat/skill side effects, mirroring the main-hand shape. No-op (and
        // `atk.last_offhand_swing_ms` stays untouched) for a ranged engagement, an unarmed/shielded
        // off-hand, or a timer not yet elapsed — so a main-hand-only attacker's tick is unchanged
        // (baseline-safe).
        if ranged.is_none() {
            resolve_offhand_swing(ctx, atk.attacker_guid, atk.target_guid, &attacker, now_ms);
            if melee.attacker_guid().find(atk.attacker_guid).is_none() {
                continue; // a lethal off-hand Duel hit removed both participants' attack rows
            }
            // The off-hand swing may have killed/disengaged `target` (or, on a lethal PLAYER off-hand
            // hit, zeroed its health) via its own writes — re-sync the in-hand snapshot the main-hand
            // path below reads/writes so it never clobbers the off-hand's kill with a stale pre-swing
            // copy. A miss/no-off-hand-weapon tick leaves `target` byte-identical (re-fetch is a no-op).
            match entities.guid().find(atk.target_guid) {
                Some(refreshed) => target = refreshed,
                None => {
                    // The off-hand's killing blow fully removed the target (a PET death delete, or a
                    // creature corpse this loop's earlier existence guard would otherwise have caught).
                    melee.attacker_guid().delete(atk.attacker_guid);
                    continue;
                }
            }
            if target.dead {
                // The off-hand swing was the killing blow — the main-hand's own swing this tick is
                // moot (there's nothing left to hit), so stop here exactly like the top-of-loop corpse
                // guard would on the NEXT tick, just one tick earlier for this row.
                continue;
            }
        }

        // Swing timer — melee uses the ATTACKER's own `base_attack_time_ms` shortened by any melee-haste
        // aura (`effective_swing_time`); ranged uses the equipped ranged weapon's `delay_ms` (bows are
        // slow, wands faster). `wrapping_sub` guards the u32 millisecond clock wrapping; the 100ms tick
        // gives 0.1s resolution. Known limitation: no ranged-haste fold in v1.
        let swing_interval = match ranged {
            Some((_, _, delay, _)) => delay,
            None => effective_swing_time(ctx, &attacker),
        };
        if atk.last_swing_ms != 0 && now_ms.wrapping_sub(atk.last_swing_ms) < swing_interval {
            continue;
        }

        // FIRE. Everything above is the SHARED eligibility gate — existence, corpse, CC, rout, the
        // ranged weapon, the movement rule, position, the off-hand's independent stream, and the swing
        // timer. What a fired swing then DOES splits cleanly in two (#370): a melee swing (the full
        // attack table, the seal / next-swing / react procs, an instant hit) and a ranged shot (a
        // reduced table, ammo, and a projectile whose damage lands on a scheduled impact). Each
        // resolver owns its own wire event and its own `last_swing_ms` stamp, and both apply damage
        // through the one shared `apply_hit` pipeline.
        match ranged {
            Some(weapon) => fire_ranged_shot(
                ctx,
                &attacker,
                &target,
                weapon,
                atk.ranged_spell_id,
                dist_sq,
                now_ms,
            ),
            None => fire_melee_swing(ctx, &attacker, &target, now_ms),
        }
    }
}

/// The POSITIONAL half of the swing gate: may `attacker` reach `target` this tick? `true` = blocked.
/// Checked in vanilla's order — max range, ranged MINIMUM range, line of sight, then facing — and
/// deliberately SIDE-EFFECT-FREE, because a block means different things to the two swing kinds and
/// that decision belongs to the caller: a melee row silently waits for the next tick, while a DUE
/// ranged row is torn down (vanilla's auto-repeat interrupt, whose row delete relays the
/// server-initiated SMSG_CANCEL_AUTO_REPEAT that drops the client's toggle). [097]/[243]
fn swing_blocked(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
    is_ranged: bool,
    dist_sq: f32,
    now_ms: u32,
) -> bool {
    // Range (re-checked each swing; the vanilla client walks into range itself). Ranged reaches
    // farther than melee.
    let range_sq = if is_ranged {
        RANGED_RANGE_SQ
    } else {
        // Melee LEEWAY: when BOTH the attacker and the target are moving (a chase), the swing reaches
        // +8/3 yd farther — so a player chasing a FLEEING mob at run-speed parity (stuck ~5yd behind)
        // can still land the hit. A standstill fight keeps the 5yd base. `last_move_ms` is the per-unit
        // move clock (player: world::move; creature: the flee/chase tick).
        let both_moving = now_ms.wrapping_sub(attacker.last_move_ms) < MELEE_LEEWAY_WINDOW_MS
            && now_ms.wrapping_sub(target.last_move_ms) < MELEE_LEEWAY_WINDOW_MS;
        if both_moving {
            // Direction-aware: only a PLAYER chasing a leg-quantized creature needs the
            // padded 9 yd; a creature attacking a continuously-streamed player uses the
            // exact classic 7.67 yd (see tables.rs).
            if attacker.is_player() {
                MELEE_RANGE_LEEWAY_SQ
            } else {
                MELEE_RANGE_LEEWAY_CREATURE_SQ
            }
        } else {
            MELEE_RANGE_SQ
        }
    };
    if dist_sq > range_sq {
        return true;
    }
    // Auto Shot / wand Shoot have a MINIMUM range (~5 yd): a target in melee range is "too close"
    // (vanilla SPELL_FAILED_TOO_CLOSE → InterruptSpell, so the player's next melee press is a clean
    // single-press swap). Melee has no minimum. [097]
    if is_ranged && dist_sq < MELEE_RANGE_SQ {
        return true;
    }
    // 243: no swinging THROUGH geometry — a mob parked at the outside of a thin wall sits
    // within the 5 yd 3D reach of a player just inside it (live find: the Rogue Wizard
    // beat a wall-separated player to death by melee). One LoS ray per due swing, both
    // directions symmetric (players can't hit through walls either). `has_los` is `true`
    // whenever nav is off — byte-identical pre-243 combat.
    if !crate::nav::has_los(
        ctx,
        attacker.map_id,
        (attacker.x, attacker.y, attacker.z),
        (target.x, target.y, target.z),
    ) {
        return true;
    }
    // Facing (PLAYERS only): vanilla refuses a swing at a target behind you — without this a fleeing
    // player auto-attacked the chaser at their back. Creatures are exempt (they auto-face their
    // victim; their `orientation` trails their movement direction, so gating them on it would break
    // legitimate mob melee).
    if attacker.is_player()
        && !crate::spell::is_facing(
            attacker.x,
            attacker.y,
            attacker.orientation,
            target.x,
            target.y,
        )
    {
        return true;
    }
    false
}

/// Fire one MAIN-HAND MELEE swing that has already passed every eligibility gate in `resolve_swing`.
/// Rolls the full vanilla attack table, folds in the melee-only procs (the Overpower react window, the
/// seal's holy portion, a queued on-next-swing strike), writes the wire events, applies the damage
/// through the SHARED [`apply_hit`] pipeline, then arms the defender's Revenge window and stamps the
/// swing timer + IN_COMBAT on both sides.
///
/// Returns early after a KILLING blow: the kill's `disengage` already freed the engagement row, so
/// there is no swing timer left to stamp and nothing left to flag in combat.
fn fire_melee_swing(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
    now_ms: u32,
) {
    let attacker_guid = attacker.guid;
    let target_guid = target.guid;
    let entities = ctx.db.game_world_entity();
    let events = ctx.db.game_combat_event();
    let melee = ctx.db.game_melee_attack();

    // The full vanilla attack table: miss / dodge / parry / glancing / block / crit / crushing.
    let (mut rolled, hit_info, blocked) = roll_swing(ctx, attacker, target);

    // React windows: an avoidance outcome ARMS a reactive ability for ~5s. A DODGE of this swing arms
    // the ATTACKER's Overpower; any avoid (dodge/parry/block) of it arms the DEFENDER's Revenge (armed
    // AFTER the damage application below, so a partial block's `update(target)` can't clobber the
    // stamp). Keyed on the HIT_* outcome, never a spell id — the cast gate (SPELL_ATTR_REQ_OVERPOWER /
    // REQ_REVENGE) reads the stamped `*_until_ms`. Stamped BEFORE the lethal fork so the window always
    // arms (the dodge already cost the attacker its swing). Ranged shots have no avoidance bands, so
    // both windows are structurally melee-only now.
    if hit_info == HIT_DODGE {
        arm_react(ctx, attacker_guid, ReactKind::Overpower);
    }

    // Seal proc (Seal of Righteousness): a LANDED swing adds the attacker's seal holy damage — the
    // swing READS the A_SEAL aura (pull model, no per-spell code). A 0-roll (miss/dodge/parry) lands
    // nothing and doesn't proc. 114: the portion (and the seal's spell id) is TRACKED so it can be
    // reported as its own yellow named line — it still folds into `rolled` (one health deduction, one
    // lethal check).
    let (mut seal_portion, mut seal_spell) = (0u32, 0u32);
    if rolled > 0 {
        let (p, s) = seal_holy_on_swing(ctx, attacker);
        seal_portion = p;
        seal_spell = s;
        rolled += seal_portion;
    }

    // On-next-swing QUEUE (Heroic Strike / Cleave): a LANDED swing adds the attacker's queued strike
    // bonus (the pull-model twin of the seal proc) — reads `attacker.next_swing_spell` and adds its
    // E_NEXT_SWING base_points. A 0-roll lands nothing. The queue is CLEARED below whenever a swing
    // FIRES (even a miss), so a missed swing still spends it (vanilla). The bonus flows through absorb
    // / lethal like the rest of `rolled`. Vanilla REPLACES the white hit — the whole landed swing IS
    // the spell (one yellow named line; live-confirmed 2026-07-11), so `queued_fired` marks the white
    // event `spell_swing` (the gateway skips its ATTACKERSTATEUPDATE) and the spell row below carries
    // the full post-mitigation damage.
    let queued_spell = attacker.next_swing_spell;
    // 114: the cost was VALIDATED at press but is CHARGED here, when the spell actually fires
    // (vanilla defers the rage deduction to the swing; a MISSED swing still charges — the spell
    // cast and missed). FIZZLE: rage fell below cost between press and swing (a Battle Shout in
    // between) → the strike doesn't fire, power is left alone, the swing stays a plain white swing,
    // and the queue is cleared below. The fizzle row carries CAST_FAIL_NO_POWER, so the caster gets
    // the cast-bar teardown AND a failed cast result naming the queued spell (vanilla's
    // SendInterrupted + SendCastResult pair). The teardown alone left the 1.12 client holding the
    // ability as its current melee spell: the button stayed lit and refused every later press.
    let mut swing_is_spell = false;
    if queued_spell != 0 && attacker.is_player() {
        let cost = ctx
            .db
            .game_spell()
            .spell_id()
            .find(queued_spell)
            .map(|h| h.cost)
            .unwrap_or(0);
        // Fresh row (read-your-writes): the seal/roll path above never wrote power.
        if let Some(mut a) = entities.guid().find(attacker_guid) {
            if a.power >= cost {
                a.power -= cost;
                entities.guid().update(a);
                swing_is_spell = true;
            } else {
                ctx.db.game_spell_cast_event().insert(SpellCastEvent {
                    is_interrupted: true, // SMSG_SPELL_FAILURE → the caster's cast bar tears down
                    failure_reason: crate::spell::CAST_FAIL_NO_POWER, // → "Not enough rage"
                    ..SpellCastEvent::signal_at(ctx, attacker, queued_spell)
                });
            }
        }
    } else if queued_spell != 0 {
        swing_is_spell = true; // non-player queued strikes (none today) fire without a pool
    }
    let queued_fired = swing_is_spell && rolled > 0;
    if queued_fired {
        rolled += queued_strike_on_swing(ctx, attacker);
    }
    // Spend the queue on ANY melee swing that fires (landed or missed) — re-fetches the row to clear it.
    // Read-your-writes safe: the bonus above already read `attacker.next_swing_spell` from the snapshot.
    clear_queued_strike(ctx, attacker_guid);

    // Stealth breaks on action: a swing that fires this tick (hit or miss) drops the attacker's
    // A_STEALTH presence — a stealthed unit that attacks is revealed. The other action chokepoint is
    // the cast path (resolve_cast_at). No-op for the common un-stealthed swing.
    crate::spell::break_stealth(ctx, attacker_guid);

    // Durability: a player's main-hand weapon wears 1 point per swing (after the roll). Creatures
    // don't wear gear.
    if attacker.is_player() {
        wear_weapon(ctx, attacker_guid);
    }

    // The SHARED modifier fold (#370): outgoing % → incoming % → absorb → godmode. `lethal` is needed
    // HERE, before the events, for the wire's `killing_blow` flag; `apply_hit` re-derives it for the
    // fork from the same shared predicate.
    let (dmg, _absorbed) = fold_incoming_damage(ctx, attacker_guid, target_guid, rolled);
    let lethal = is_lethal(target.health, dmg);

    // One swing event per swing; `lethal` (killing_blow) also tells the relay to send
    // SMSG_ATTACKSTOP so the attacker leaves combat stance. Logged before any teardown so the
    // event doesn't depend on whether the target row survives this tick.
    // 114: the seal's holy portion gets its own named yellow line (the proc insert below), so the
    // WHITE event reports the swing minus it. A fired queued strike goes further — the WHOLE swing
    // is the spell (spell_swing → the gateway skips this event's ATTACKERSTATEUPDATE; the spell row
    // below carries the full damage). `dmg` (the total) still drives health/lethal/rage; the seal
    // subtraction is display-only (a pre-modifier flat, so a modified/absorbed swing's split is
    // approximate — totals stay exact).
    events.insert(CombatEvent {
        damage: dmg.saturating_sub(seal_portion),
        hit_info,
        killing_blow: lethal,
        blocked_amount: blocked,
        spell_swing: swing_is_spell,
        // ranged_spell_id / ammo_display_id / impact_delay_ms stay at the baseline's 0: a melee
        // engagement always carries 0 (that is what makes it melee), consumes no ammo, and lands
        // instantly (only a projectile carries travel time).
        ..CombatEvent::signal_at(ctx, attacker, target_guid)
    });
    // 114 FIX (a): the queued on-next-swing strike FIRES with this swing — emit its cast event NOW
    // (never at queue time). is_completion=true rides the timed-completion relay shape, which is
    // exactly the vanilla on-next-swing fire: CAST_RESULT(OK) to the caster (releases the client's
    // pending cast = un-lights the button) + SMSG_SPELL_GO alone (no second START) + the yellow
    // "Heroic Strike hits ..." damage log carrying the WHOLE landed swing (weapon roll + bonus,
    // post-mitigation; the seal's portion stays on its own line). A MISSED swing still emits the
    // row (damage 0, no log) — the GO must fire or the client's button stays lit forever.
    if swing_is_spell {
        ctx.db.game_spell_cast_event().insert(SpellCastEvent {
            target_guid,
            is_completion: true,
            damage: if queued_fired {
                dmg.saturating_sub(seal_portion)
            } else {
                0
            },
            // school stays at the baseline's 0 (physical — HS/Cleave are weapon strikes).
            is_crit: hit_info == HIT_CRIT,
            // The swing outcome — on a 0-damage fire the relay shapes the GO miss list from it
            // (yellow "Heroic Strike missed/was dodged/was parried", not a white MISS line).
            swing_hit_info: hit_info,
            ..SpellCastEvent::signal_at(ctx, attacker, queued_spell)
        });
    }
    // 114 FIX (b): the seal's holy portion — a log-only row (is_proc_log): the gateway sends ONLY
    // SMSG_SPELLNONMELEEDAMAGELOG named after the seal spell (yellow "Seal of Righteousness hits
    // ... Holy"), never START/GO (nothing casts; the seal aura is already up — vanilla shape).
    if seal_portion > 0 {
        ctx.db.game_spell_cast_event().insert(SpellCastEvent {
            target_guid,
            damage: seal_portion,
            school: 1, // holy (school_mask 2 → index 1, the mask→index rule in resolve_cast_at)
            is_proc_log: true,
            ..SpellCastEvent::signal_at(ctx, attacker, seal_spell)
        });
    }

    // The SHARED application (#370): rage both ways, weapon/defense skill-ups, the lethal fork through
    // kill_player/kill_creature, the health write, break-on-damage, and threat.
    if apply_hit(
        ctx,
        attacker_guid,
        target_guid,
        dmg,
        Hit::weapon(HitSource::MainHand, hit_info == HIT_CRIT),
    )
    .combat_ended()
    {
        return; // the kill's `disengage` freed the row — nothing left to stamp
    }

    // Defender's Revenge window: armed AFTER the damage write above so a partial block (dmg>0,
    // non-lethal) cannot clobber the stamp with a pre-stamp `target` snapshot. `arm_react` re-fetches
    // the defender fresh, so it works for both the dmg==0 (dodge/parry) and dmg>0 (partial block)
    // paths. A lethal block returned earlier — a dead defender needs no window.
    if matches!(hit_info, HIT_DODGE | HIT_PARRY | HIT_BLOCK) {
        arm_react(ctx, target_guid, ReactKind::Revenge);
    }

    // Re-fetch fresh before the tail write: `resolve_offhand_swing` (run by the caller earlier this
    // tick) may have persisted its own `last_offhand_swing_ms` stamp on this same row.
    if let Some(mut fresh) = melee.attacker_guid().find(attacker_guid) {
        fresh.last_swing_ms = now_ms;
        melee.attacker_guid().update(fresh);
    }

    // A swing FIRED this tick (hit or miss) → both combatants are IN COMBAT. Set the flag at the tail
    // so it lands AFTER every entity write above; `enter_combat` re-fetches fresh.
    enter_combat(ctx, attacker_guid);
    enter_combat(ctx, target_guid);
}

/// Fire one RANGED shot (Auto Shot 75 / wand Shoot 5019) that has already passed every eligibility
/// gate in `resolve_swing`. Consumes ammo, rolls the REDUCED ranged attack table (miss / crit /
/// normal — a shot can't be dodged, parried or blocked here), folds the damage through the SHARED
/// modifier chain, and launches a PROJECTILE: the wire event relays the SMSG_SPELL_GO (arrow) NOW and
/// carries `impact_delay_ms`, while the scheduled `ranged_impact` applies the frozen damage through
/// the SAME [`apply_hit`] pipeline when the arrow actually lands — so the number, the health drop and
/// the projectile arrive together (user bug: "damage lands earlier than the projectile"). Lethality is
/// therefore decided AT IMPACT, which is why this event never claims `killing_blow`. [097]
///
/// Out of ammo ends the engagement: the row delete relays the server-initiated
/// SMSG_CANCEL_AUTO_REPEAT that drops the client's toggle.
fn fire_ranged_shot(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
    weapon: (u32, u32, u32, u8),
    ranged_spell_id: u32,
    dist_sq: f32,
    now_ms: u32,
) {
    use crate::items::weapon_subclass as ws;
    let (dmin, dmax, delay, subclass) = weapon;
    let attacker_guid = attacker.guid;
    let target_guid = target.guid;
    let melee = ctx.db.game_melee_attack();
    let launcher = matches!(subclass, ws::BOW | ws::GUN | ws::CROSSBOW);

    // Ammo: reaching here means a shot FIRES this tick. A launcher consumes 1 arrow/bullet from the
    // bag and stamps its display id onto the event (so the gateway renders the arrow projectile);
    // out of ammo ends the engagement, which stops the client's auto-shoot too. A wand consumes
    // nothing (display 0). Consumed on the shot regardless of hit/miss, like vanilla.
    let ammo_display_id: u32 = if launcher {
        match find_ammo(ctx, attacker_guid) {
            Some(mut a) => {
                let disp = ctx
                    .db
                    .game_item_template()
                    .entry()
                    .find(a.entry)
                    .map(|t| t.display_id)
                    .unwrap_or(0);
                a.stack_count -= 1;
                if a.stack_count == 0 {
                    ctx.db.game_item_instance().guid().delete(a.guid);
                } else {
                    ctx.db.game_item_instance().guid().update(a);
                }
                disp
            }
            None => {
                melee.attacker_guid().delete(attacker_guid);
                return;
            }
        }
    } else {
        0 // wand: flat damage, no ammo
    };

    // Ranged AP fold: a launcher scales with the shooter's Agility-derived ranged attack power, folded
    // into the weapon range by speed exactly like melee; a wand is FLAT weapon damage (no ranged AP in
    // vanilla). Known limitation: no ranged-AP aura (e.g. Aspect of the Hawk) — we have none.
    let (rdmin, rdmax) = if launcher && attacker.is_player() {
        let rap = ranged_attack_power(effective_agility(ctx, attacker), attacker.level);
        weapon_swing_range_ap(rap, dmin, dmax, delay)
    } else {
        (dmin, dmax)
    };
    let (rolled, hit_info) = roll_ranged_swing(ctx, attacker, target, rdmin, rdmax);

    // Stealth breaks on action — a shot that fires (hit or miss) reveals the shooter, exactly like a
    // melee swing.
    crate::spell::break_stealth(ctx, attacker_guid);

    // The SHARED modifier fold (#370). Vanilla folds absorb at IMPACT, but freezing the whole chain
    // here keeps the delayed damage LOG equal to what actually lands; the ≤1s divergence window is
    // noise. The godmode zero is nonetheless RE-CHECKED at impact (a target can toggle it mid-flight).
    let (dmg, _absorbed) = fold_incoming_damage(ctx, attacker_guid, target_guid, rolled);

    let speed = if subclass == ws::WAND {
        WAND_PROJECTILE_SPEED
    } else {
        AUTO_SHOT_PROJECTILE_SPEED
    };
    let travel_ms = crate::spell::projectile_travel_ms(dist_sq.sqrt(), speed);
    ctx.db.game_combat_event().insert(CombatEvent {
        damage: dmg,
        hit_info,
        ranged_spell_id,
        ammo_display_id,
        impact_delay_ms: travel_ms,
        // killing_blow stays at the baseline's false (decided at IMPACT — health can move
        // mid-flight); blocked_amount stays 0 (a shot can't be blocked in this engine).
        ..CombatEvent::signal_at(ctx, attacker, target_guid)
    });
    // A miss / fully-absorbed shot schedules nothing (the GO's miss list, or the absence of a damage
    // log, is the whole story).
    if dmg > 0 {
        let land_at = ctx
            .timestamp
            .checked_add(TimeDuration::from_micros((travel_ms as i64) * 1000))
            .unwrap_or(ctx.timestamp);
        ctx.db
            .game_ranged_impact_schedule()
            .insert(RangedImpactSchedule {
                scheduled_id: 0,
                scheduled_at: ScheduleAt::Time(land_at),
                attacker_guid,
                target_guid,
                damage: dmg,
            });
    }
    // Same fresh-row stamp discipline as the melee tail (the off-hand never runs on a ranged row, but
    // the re-fetch is the file's established pattern for a row written mid-tick).
    if let Some(mut fresh) = melee.attacker_guid().find(attacker_guid) {
        fresh.last_swing_ms = now_ms;
        melee.attacker_guid().update(fresh);
    }
}

/// Scheduled RANGED-projectile impact (097; scheduler-only): the arrow/bullet/bolt lands — apply the
/// launch-frozen post-mitigation damage through the SAME shared [`apply_hit`] pipeline the melee swing
/// uses (health/lethal via the shared kill helpers, rage both ways, weapon/defense skill-ups,
/// break-on-damage, threat), then stamp IN_COMBAT on both sides. Guards re-checked at landing: either
/// side gone or the target already dead → the shot fizzles silently (the client saw the arrow; vanilla
/// eats mid-flight kills the same way). Lethality is decided HERE from fresh health, never at launch.
#[reducer]
pub fn ranged_impact(ctx: &ReducerContext, shot: RangedImpactSchedule) {
    if ctx.sender() != ctx.database_identity() {
        return;
    }
    let entities = ctx.db.game_world_entity();
    let Some(attacker) = entities.guid().find(shot.attacker_guid) else {
        return; // the shooter left the world mid-flight
    };
    let Some(target) = entities.guid().find(shot.target_guid) else {
        return;
    };
    if target.dead || shot.damage == 0 {
        return;
    }
    if !crate::combat::may_harm(ctx, &attacker, &target) {
        return; // Duel or faction authorization changed while the projectile was in flight
    }
    // GM playtest godmode (work-item 223's `.god`): the ONE modifier this path re-evaluates at impact.
    // The rest of the chain (outgoing %, damage-taken %, absorb) was folded and FROZEN at launch by
    // `fire_ranged_shot`, so the delayed damage LOG equals what actually lands — but godmode can be
    // toggled on DURING the arrow's flight, and re-checking only the frozen value would let a delayed
    // arrow land damage that a melee swing at the same instant would zero (issue #361). `false` for a
    // non-godmode target, so this is byte-identical for them.
    let dmg = if target.godmode { 0 } else { shot.damage };
    // The SHARED pipeline (#370) — the same one the main-hand and off-hand swings route through, so a
    // shot can never again drift from a swing (a godmode-zeroed hit is now a full no-op here too,
    // instead of running the survivor path with a 0 damage value).
    let outcome = apply_hit(
        ctx,
        shot.attacker_guid,
        shot.target_guid,
        dmg,
        Hit::weapon(HitSource::Ranged, false),
    );
    if outcome.duel_completed {
        return;
    }
    enter_combat(ctx, shot.attacker_guid);
    enter_combat(ctx, shot.target_guid);
}

#[cfg(test)]
mod damage_pipeline_drift_tests {
    // #361 found TWO live divergences in what was then a copy-pasted post-roll damage pipeline —
    // off-hand swings ignored Disarm, ranged impacts ignored godmode — and pinned each fix in place.
    // #370 removed the copies: there is now ONE pipeline (`fold_incoming_damage` → `apply_hit`) that
    // every damaging path routes through, so the drift class those two tests guard against can no
    // longer be introduced one resolver at a time. These tests therefore moved UP a level: instead of
    // pinning each fix in each copy, they pin that every resolver still goes through the one
    // chokepoint (plus the two #361 guards that remain genuinely resolver-local).
    //
    // There is no `ReducerContext` harness in this crate by design (see `test_scan`'s doc comment /
    // playbook §7), so — same as the other chokepoint tests in this file — these pin the wiring's
    // PRESENCE in the reducers' own source text. They prove the call is there, not that the pipeline
    // behaves correctly in isolation; the pure pieces (`is_lethal`, the rage/armor/attack-table math)
    // are covered by their own direct tests. Every scan goes through `code_of`, never `body_of`, so a
    // needle planted in a comment can't satisfy it.
    use crate::test_scan::code_of;

    /// Every resolver that applies damage, keyed by the source file it lives in.
    const RESOLVERS: [(&str, &str); 4] = [
        ("combat/swing.rs", "fn fire_melee_swing("),
        ("combat/swing.rs", "fn resolve_offhand_swing("),
        ("combat/swing.rs", "pub fn ranged_impact("),
        ("spell/effects.rs", "pub(crate) fn apply_target_damage("),
    ];

    fn source(file: &str) -> &'static str {
        match file {
            "combat/swing.rs" => include_str!("swing.rs"),
            "spell/effects.rs" => include_str!("../spell/effects.rs"),
            other => panic!("no source registered for `{other}`"),
        }
    }

    #[test]
    fn every_damage_resolver_routes_through_apply_hit() {
        for (file, sig) in RESOLVERS {
            let body = code_of(source(file), sig);
            assert!(
                body.contains("apply_hit("),
                "`{sig}` in {file} no longer applies its damage through the shared `apply_hit` \
                 pipeline. That pipeline (rage both ways, skill-ups, the lethal fork through \
                 kill_player/kill_creature, the health write, break-on-damage, threat) exists exactly \
                 once precisely because hand-maintained copies of it drifted twice (#361). Body \
                 was:\n{body}"
            );
        }
    }

    /// The source argument is not decoration: it is the combat EVENT the Proc engine fires off, and a
    /// resolver that named the wrong one would fire the wrong procs (an off-hand swing that claimed to
    /// be a main-hand one would never feed an off-hand-only proc). `apply_target_damage` is the odd one
    /// out on purpose — it takes the hit from its caller and must forward it verbatim, never invent one.
    #[test]
    fn each_weapon_resolver_names_the_proc_event_its_hit_raises() {
        for (sig, event) in [
            ("fn fire_melee_swing(", "HitSource::MainHand"),
            ("fn resolve_offhand_swing(", "HitSource::OffHand"),
            ("pub fn ranged_impact(", "HitSource::Ranged"),
        ] {
            let body = code_of(source("combat/swing.rs"), sig);
            assert!(
                body.contains(event),
                "`{sig}` no longer names `{event}` on its hit — the Proc engine fires the combat \
                 event this argument selects. Body was:\n{body}"
            );
        }
        let spell = code_of(
            source("spell/effects.rs"),
            "pub(crate) fn apply_target_damage(",
        );
        assert!(
            spell.contains("apply_hit(ctx, caster_guid, target_guid, dmg, hit)"),
            "`apply_target_damage` must forward its caller's hit verbatim — manufacturing one here \
             would relabel every spell effect's proc event. Body was:\n{spell}"
        );
    }

    #[test]
    fn no_resolver_reaches_the_kill_chokepoints_behind_apply_hit() {
        for (file, sig) in RESOLVERS {
            let body = code_of(source(file), sig);
            for direct in ["kill_creature(", "kill_player("] {
                assert!(
                    !body.contains(direct),
                    "`{sig}` in {file} calls `{direct}` directly again. The lethal fork — including \
                     WHO gets kill credit (a pet credits its owner) and the spell path's \
                     floor-a-player-at-1-hp exception — belongs to `apply_hit` alone; a second copy \
                     of it is how the credit rules drift apart. Body was:\n{body}"
                );
            }
        }
    }

    #[test]
    fn every_freshly_rolled_hit_folds_through_the_shared_modifier_chain() {
        // The ranged IMPACT is deliberately absent: its damage was folded (and frozen) at launch by
        // `fire_ranged_shot`, which is in this list. Everything that rolls a NEW number folds it here.
        for (file, sig) in [
            ("combat/swing.rs", "fn fire_melee_swing("),
            ("combat/swing.rs", "fn resolve_offhand_swing("),
            ("combat/swing.rs", "fn fire_ranged_shot("),
            ("spell/effects.rs", "pub(crate) fn apply_target_damage("),
        ] {
            let body = code_of(source(file), sig);
            assert!(
                body.contains("fold_incoming_damage("),
                "`{sig}` in {file} no longer folds its rolled damage through \
                 `fold_incoming_damage` — the outgoing %, damage-taken %, absorb and godmode chain. \
                 Re-inlining any of those four steps re-creates the copy that drifted in #361. Body \
                 was:\n{body}"
            );
            assert!(
                !body.contains("absorb_incoming("),
                "`{sig}` in {file} folds absorb itself again instead of leaving it to \
                 `fold_incoming_damage`. Body was:\n{body}"
            );
        }
    }

    #[test]
    fn offhand_swing_checks_disarm_before_rolling_its_range() {
        // #361, still resolver-local: Disarm strips the MAIN hand inside `swing_range_ctx`, and the
        // off-hand derives its own range, so the gate has to be read here too.
        let body = code_of(include_str!("swing.rs"), "fn resolve_offhand_swing(");
        assert!(
            body.contains("is_disarmed"),
            "resolve_offhand_swing no longer reads is_disarmed — a Disarmed dual-wielder would keep \
             swinging the off-hand at full value while swing_range_ctx strips the main hand. Body \
             was:\n{body}"
        );
    }

    #[test]
    fn ranged_impact_re_checks_godmode_at_impact_time() {
        // #361, still resolver-local: the rest of the modifier chain is frozen at LAUNCH, so this is
        // the one fold the impact must re-evaluate (godmode can be toggled during the arrow's flight).
        let body = code_of(include_str!("swing.rs"), "pub fn ranged_impact(");
        assert!(
            body.contains("target.godmode"),
            "ranged_impact no longer re-checks the target's godmode flag at impact — a delayed arrow \
             would land damage on a target that toggled godmode on mid-flight, which a melee swing at \
             the same instant would zero. Body was:\n{body}"
        );
    }

    #[test]
    fn the_ranged_teardown_rule_is_written_once() {
        // #370: the "a blocker on a DUE ranged shot INTERRUPTS the auto-repeat loop" teardown used to
        // be repeated at each of the four positional gates (range / minimum range / LoS / facing).
        // Those gates now live in the side-effect-free `swing_blocked`, so `resolve_swing` states the
        // teardown once. Pinned by absence: the gates' own predicates must not reappear inline.
        let body = code_of(include_str!("swing.rs"), "fn resolve_swing(");
        for gate in [
            "has_los(",
            "is_facing(",
            "RANGED_RANGE_SQ",
            "MELEE_RANGE_LEEWAY_SQ",
        ] {
            assert!(
                !body.contains(gate),
                "`resolve_swing` inlines the positional gate `{gate}` again instead of delegating to \
                 `swing_blocked`. Each inlined gate needs its own `if ranged_due {{ delete }}` \
                 teardown, and that is exactly the repetition #370 removed. Body was:\n{body}"
            );
        }
        assert_eq!(
            body.matches("if ranged_due").count(),
            2,
            "`resolve_swing` should branch on `ranged_due` exactly twice: once to DEFER a moving \
             shooter's shot (the Auto Shot re-arm) and once for the positional gate's teardown. A \
             third occurrence means a gate got inlined again, each inline copy bringing its own \
             teardown back with it. Body was:\n{body}"
        );
    }
}
