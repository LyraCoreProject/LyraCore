//! The consolidated post-publish repair pass (#378) — the one reducer the publish script calls after
//! every deploy to re-seed/rearm everything `seed::init` would have inserted on a fresh database but
//! that a plain auto-migrate republish silently leaves absent.

use spacetimedb::{log, reducer, ReducerContext, ScheduleAt, Table, TimeDuration};

use crate::spell::stacking::game_spell_group;
use crate::{
    game_auction, game_auction_expiry, game_aura_schedule, game_breath_schedule, game_character,
    game_createinfo_spell, game_creature_move_schedule, game_duel_schedule, game_faction,
    game_gateway_lease_reaper_schedule, game_ground_area_schedule, game_instance_reaper_schedule,
    game_item_template, game_motion_publish_schedule, game_pet_care_schedule, game_spell,
    game_talent, AuctionExpiry, AuraSchedule, BreathSchedule, CreatureMoveSchedule, DuelSchedule,
    GroundAreaSchedule, PetCareSchedule,
};

/// Consolidated post-publish repair pass (#378). SpacetimeDB's `init` reducer runs ONLY on a
/// database's first-ever publish, never again on a plain (auto-migrate) republish — so every
/// seed/fixture/schedule row `seed::init` inserts is silently ABSENT on an already-migrated live DB
/// until something re-seeds it by hand. That "something" used to be 17 near-identical
/// `debug_seed_*`/`debug_ensure_*`/`debug_rearm_*` reducers, one per fixture family — a model that
/// failed operationally at least four times (aura schedule, ground-area schedule, regen fixture,
/// catalogue drift #85), and forced `debug_seed_scenario_fixtures` to bolt two extra seeders onto
/// itself as a stopgap when a live suite hard-failed on a missing fixture. Every underlying seeder
/// is idempotent-if-absent by construction (each deleted reducer documented the same precedent), so
/// running all of them unconditionally on every publish — fresh or auto-migrated — is always safe,
/// and "which of the N do I call after this publish?" collapses to "just this one".
///
/// Gated by `require_operator` (the strictest of the reducers it replaces, formerly
/// `debug_rearm_instance_reaper`'s gate alone) rather than left open like most of its predecessors:
/// this is meant to run exactly once per publish, driven by the same CLI/deploy identity that ran
/// `spacetime publish` and (earlier) `claim_operator` — see `scripts/publish-module.sh`, which calls
/// it automatically after every successful publish. On a database's FIRST-EVER publish `init` has
/// already seeded everything and the operator has not been claimed yet, so this call is EXPECTED to
/// no-op-fail with "operator not claimed" there; the publish script treats that as non-fatal.
///
/// Stamps ONE `game_import_meta` row (family `"debug_repair_after_publish"`, work-item 216
/// provenance) with the total row count across every seeded/rearmed family, rather than the 13
/// separate per-family stamps the old reducers wrote.
#[reducer]
pub fn debug_repair_after_publish(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;

    // --- seeders (each idempotent: inserts only rows that are absent) ---
    crate::talent::seed_talents(ctx);
    let talents = ctx.db.game_talent().count();

    // formerly `debug_seed_gm_tester`: (re)grant the seeded Tester (guid 1) `gm_level` 3 — needed on
    // an already-migrated dev DB where `seed::init` did NOT re-run (work-item 223: the `gm_level`
    // column auto-migrates existing rows to 0). No-op if the Tester character row is gone.
    if let Some(mut c) = ctx.db.game_character().guid().find(1) {
        c.gm_level = 3;
        ctx.db.game_character().guid().update(c);
    }

    crate::seed::seed_createinfo_spells(ctx);
    let createinfo = ctx.db.game_createinfo_spell().count();

    crate::seed::seed_spell_groups(ctx);
    let spell_groups = ctx.db.game_spell_group().count();

    // The live stacking-family probe needs its four family members present on an already-migrated
    // database, where `init` never re-runs.
    crate::seed::seed_stacking_probe_fixture(ctx);

    crate::seed::seed_pw_shield_fixture(ctx);
    // Weakened Soul 6788, Test PW:Shield 50072
    let pw_shield = [6788u32, 50072u32]
        .iter()
        .filter(|id| ctx.db.game_spell().spell_id().find(**id).is_some())
        .count() as u64;

    crate::seed::seed_fixture_catalogue(ctx);
    // Tempered Blade 5090050 + Tough Jerky 5090052 + fixture faction 50900 (issue #85)
    let fixture_catalogue = [crate::seed::FIXTURE_BLADE, crate::seed::FIXTURE_JERKY]
        .iter()
        .filter(|e| ctx.db.game_item_template().entry().find(**e).is_some())
        .count() as u64
        + ctx
            .db
            .game_faction()
            .faction_id()
            .find(crate::seed::FIXTURE_FACTION)
            .is_some() as u64;

    crate::seed::seed_soul_shard_item(ctx);
    let soul_shard = ctx
        .db
        .game_item_template()
        .entry()
        .find(crate::combat::SOUL_SHARD_ENTRY)
        .is_some() as u64;

    crate::seed::seed_drain_soul_fixture(ctx);
    let drain_soul = ctx
        .db
        .game_spell()
        .spell_id()
        .find(crate::combat::DRAIN_SOUL_SPELL_ID)
        .is_some() as u64;

    crate::seed::seed_mana_burn_fixture(ctx);
    let mana_burn = ctx.db.game_spell().spell_id().find(8129u32).is_some() as u64;

    crate::seed::seed_stealth_fixture(ctx);
    let stealth = ctx.db.game_spell().spell_id().find(1784u32).is_some() as u64;

    crate::seed::seed_frost_armor_fixture(ctx);
    // Chilled 6136, Frost Armor 168
    let frost_armor = [6136u32, 168u32]
        .iter()
        .filter(|id| ctx.db.game_spell().spell_id().find(**id).is_some())
        .count() as u64;

    crate::seed::seed_demon_skin_fixture(ctx);
    let demon_skin = ctx.db.game_spell().spell_id().find(696u32).is_some() as u64;

    crate::seed::seed_regen_fixture(ctx);
    // Test Regeneration 50137 (the combat-regen probe's kind-169 A_COMBAT_HEALTH_REGEN_PCT source)
    let regen = ctx.db.game_spell().spell_id().find(50137u32).is_some() as u64;

    // The land-mount fixture: Riding skill line 762, Test Riding Horse 50310 + its two effects, the
    // skill-ability row that makes it trainable, and Test Dazed 50311. `seed_fixture_catalogue` above
    // restores the reins ITEM, so without this the item would survive a republish pointing at a spell
    // that no longer exists.
    crate::seed::seed_mount_fixture(ctx);
    let mount_fixture = [
        crate::seed::FIXTURE_MOUNT_SPELL,
        crate::seed::FIXTURE_DAZED_SPELL,
    ]
    .iter()
    .filter(|id| ctx.db.game_spell().spell_id().find(**id).is_some())
    .count() as u64;

    // --- schedule ensures (idempotent: no-op if a row is already present) ---
    // formerly `debug_ensure_aura_schedule`: matches the 1s interval in `seed::init`.
    let aura_schedule = if ctx.db.game_aura_schedule().iter().next().is_some() {
        0
    } else {
        ctx.db.game_aura_schedule().insert(AuraSchedule {
            scheduled_id: 0,
            scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(1_000_000)),
        });
        1
    };

    let breath_schedule = if ctx.db.game_breath_schedule().iter().next().is_some() {
        0
    } else {
        ctx.db.game_breath_schedule().insert(BreathSchedule {
            scheduled_id: 0,
            scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(1_000_000)),
        });
        1
    };

    let duel_schedule = if ctx.db.game_duel_schedule().iter().next().is_some() {
        0
    } else {
        ctx.db.game_duel_schedule().insert(DuelSchedule {
            scheduled_id: 0,
            scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(
                crate::duel::DUEL_TICK_MICROS,
            )),
        });
        1
    };

    // formerly `debug_ensure_ground_area_schedule`: matches the 500ms interval in `seed::init`.
    let ground_area_schedule = if ctx.db.game_ground_area_schedule().iter().next().is_some() {
        0
    } else {
        ctx.db
            .game_ground_area_schedule()
            .insert(GroundAreaSchedule {
                scheduled_id: 0,
                scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(500_000)),
            });
        1
    };

    // #461: ensure — never REARM — the 20 Hz movement-republish schedule. Ensure, because
    // `scheduled_at` IS the cadence knob (`set_motion_tick_ms`), and a publish must not silently
    // undo an operator's retune. Absent, movement stages into the private table and never relays,
    // so this is the row whose absence is loudest.
    let motion_schedule = if ctx
        .db
        .game_motion_publish_schedule()
        .iter()
        .next()
        .is_some()
    {
        0
    } else {
        ctx.db
            .game_motion_publish_schedule()
            .insert(crate::motion::MotionPublishSchedule {
                scheduled_id: 0,
                scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(
                    crate::motion::MOTION_TICK_MICROS,
                )),
            });
        1
    };

    // Hunter care is an authoritative module timer. Ensure it after an auto-migrate publish, where
    // `init` does not run, without disturbing an already-armed row.
    let pet_care_schedule = if ctx.db.game_pet_care_schedule().iter().next().is_some() {
        0
    } else {
        ctx.db.game_pet_care_schedule().insert(PetCareSchedule {
            scheduled_id: 0,
            scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(
                crate::creatures::CARE_INTERVAL_MICROS,
            )),
        });
        1
    };

    // --- rearms (idempotent: always replace with the canonical row) ---
    // formerly `debug_rearm_creature_tick`: re-arm the GLOBAL creature movement tick to 0.5s. Work-item
    // 229: DEDICATED per-instance rows (armed via `debug_arm_instance_tick`) are deliberately left
    // alone — this only ever touches the catch-all row, and doubles as the recovery path if that
    // load-bearing row was ever deleted (every instance lacking a dedicated row stops ticking
    // entirely, including the open world, without it).
    let move_sched = ctx.db.game_creature_move_schedule();
    let stale_move: Vec<u64> = move_sched
        .iter()
        .filter(|r| r.instance_id == crate::creatures::GLOBAL_TICK_INSTANCE)
        .map(|r| r.scheduled_id)
        .collect();
    for id in stale_move {
        move_sched.scheduled_id().delete(id);
    }
    move_sched.insert(CreatureMoveSchedule {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(500_000)),
        instance_id: crate::creatures::GLOBAL_TICK_INSTANCE,
    });

    // formerly `debug_rearm_instance_reaper`: re-seed the 60s instance-reaper schedule row.
    let reaper_sched = ctx.db.game_instance_reaper_schedule();
    let stale_reaper: Vec<u64> = reaper_sched.iter().map(|r| r.scheduled_id).collect();
    for id in stale_reaper {
        reaper_sched.scheduled_id().delete(id);
    }
    reaper_sched.insert(crate::instance::InstanceReaperSchedule {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(
            crate::instance::INSTANCE_REAPER_INTERVAL_MICROS,
        )),
    });

    // #468 stage 4a: re-arm the gateway lease reaper (bounded ghost lifetime for sessions riding
    // the shared connection). Rearm-not-ensure is safe here — unlike the motion tick there is no
    // operator tuning knob to preserve; the canonical interval is the only interval.
    let lease_sched = ctx.db.game_gateway_lease_reaper_schedule();
    let stale_lease: Vec<u64> = lease_sched.iter().map(|r| r.scheduled_id).collect();
    for id in stale_lease {
        lease_sched.scheduled_id().delete(id);
    }
    lease_sched.insert(crate::gw::GatewayLeaseReaperSchedule {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(crate::gw::LEASE_REAP_MICROS)),
    });

    // One-shot Auction expiries are created with their Auctions, not seeded globally. A publish
    // must preserve them; if an older/broken publication left one absent, restore only that missing
    // row at the Auction's original deadline. Never re-arm an existing row or move its deadline.
    let mut auction_expiries = 0u64;
    for auction in ctx.db.game_auction().iter() {
        if ctx
            .db
            .game_auction_expiry()
            .auction_id()
            .find(auction.id)
            .is_none()
        {
            ctx.db.game_auction_expiry().insert(AuctionExpiry {
                scheduled_id: 0,
                scheduled_at: ScheduleAt::Time(auction.expires_at),
                auction_id: auction.id,
            });
            auction_expiries += 1;
        }
    }

    // row_count: total rows/rearms across every family this pass touched (work-item 216 provenance
    // stamp) — one stamp for the whole repair pass, replacing the 13 separate per-family stamps the
    // deleted reducers wrote. The trailing `+ 2` is the two schedule rows this always (re)arms
    // unconditionally (creature tick + instance reaper), which have no "was it already present"
    // signal worth stamping separately.
    let total = talents
        + createinfo
        + spell_groups
        + pw_shield
        + fixture_catalogue
        + soul_shard
        + drain_soul
        + mana_burn
        + stealth
        + frost_armor
        + demon_skin
        + regen
        + mount_fixture
        + aura_schedule
        + breath_schedule
        + duel_schedule
        + ground_area_schedule
        + motion_schedule
        + pet_care_schedule
        + auction_expiries
        + 2;
    crate::import_meta::stamp(ctx, "debug_repair_after_publish", "", "", total);
    log::info!("debug_repair_after_publish: repaired {total} fixture/schedule row(s), including missing Auction expiries");
    Ok(())
}

#[cfg(test)]
mod pet_care_schedule_tests {
    #[test]
    fn post_publish_repair_ensures_the_authoritative_pet_care_clock() {
        let body = crate::test_scan::code_of(
            include_str!("repair.rs"),
            "pub fn debug_repair_after_publish(",
        );
        assert!(body.contains("game_pet_care_schedule().iter().next().is_some()"));
        assert!(body.contains("CARE_INTERVAL_MICROS"));
    }
}
