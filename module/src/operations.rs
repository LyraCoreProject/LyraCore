use spacetimedb::{log, reducer, ReducerContext, ScheduleAt, Table, TimeDuration};

use crate::spell::stacking::game_spell_group;
use crate::{
    game_auction, game_auction_expiry, game_aura, game_aura_schedule, game_breath_schedule,
    game_character, game_createinfo_spell, game_creature_move_schedule, game_duel_schedule,
    game_faction, game_gateway_lease_reaper_schedule, game_ground_area_schedule,
    game_instance_reaper_schedule, game_item_template, game_motion_publish_schedule,
    game_pet_care_schedule, game_spell, game_talent, AuctionExpiry, AuraSchedule, BreathSchedule,
    CreatureMoveSchedule, DuelSchedule, GroundAreaSchedule, PetCareSchedule,
};

#[reducer]
#[allow(clippy::too_many_lines)]
pub fn debug_repair_after_publish(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;

    crate::talent::seed_talents(ctx);
    let talents = ctx.db.game_talent().count();

    if let Some(mut c) = ctx.db.game_character().guid().find(1) {
        c.gm_level = 3;
        ctx.db.game_character().guid().update(c);
    }

    crate::seed::seed_createinfo_spells(ctx);
    let createinfo = ctx.db.game_createinfo_spell().count();

    let lesser_heal_target = crate::seed::repair_lesser_heal_target(ctx);

    crate::seed::seed_spell_groups(ctx);
    crate::items::seed_compatibility_enchantments(ctx);
    let spell_groups = ctx.db.game_spell_group().count();

    crate::seed::seed_stacking_probe_fixture(ctx);

    crate::seed::seed_pw_shield_fixture(ctx);
    let pw_shield = [6788u32, 50072u32]
        .iter()
        .filter(|id| ctx.db.game_spell().spell_id().find(**id).is_some())
        .count() as u64;

    crate::seed::seed_fixture_catalogue(ctx);
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
    crate::seed::seed_test_proc_fixtures(ctx);
    let frost_armor = [
        6136u32,
        168u32,
        crate::seed::TEST_PROC_MARK,
        crate::seed::TEST_PROC_COIN,
        crate::seed::TEST_PROC_CHARGES,
        crate::seed::TEST_PROC_COOLDOWN,
        crate::seed::TEST_PROC_PPM,
        crate::seed::TEST_PROC_ZAP,
    ]
    .iter()
    .filter(|id| ctx.db.game_spell().spell_id().find(**id).is_some())
    .count() as u64;

    crate::seed::seed_demon_skin_fixture(ctx);
    let demon_skin = ctx.db.game_spell().spell_id().find(696u32).is_some() as u64;

    crate::seed::seed_regen_fixture(ctx);
    let regen = ctx.db.game_spell().spell_id().find(50137u32).is_some() as u64;

    crate::seed::seed_mount_fixture(ctx);
    let mount_fixture = [
        crate::seed::FIXTURE_MOUNT_SPELL,
        crate::seed::FIXTURE_DAZED_SPELL,
    ]
    .iter()
    .filter(|id| ctx.db.game_spell().spell_id().find(**id).is_some())
    .count() as u64;

    let weather_climate = crate::weather::seed_weather_weights(ctx);

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

    // Ensure rather than rearm because scheduled_at preserves the Operator's chosen motion cadence.
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

    let lease_sched = ctx.db.game_gateway_lease_reaper_schedule();
    let stale_lease: Vec<u64> = lease_sched.iter().map(|r| r.scheduled_id).collect();
    for id in stale_lease {
        lease_sched.scheduled_id().delete(id);
    }
    lease_sched.insert(crate::gw::GatewayLeaseReaperSchedule {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(crate::gw::LEASE_REAP_MICROS)),
    });

    crate::weather::rearm_weather_schedule(ctx);

    crate::meeting_stone::rearm_reminder_schedule(ctx);

    let (legacy_auction_mail, _unmapped_auction_mail) =
        crate::auction::repair_legacy_auction_mail(ctx);

    // Preserve one-shot Auction expiries; restore only missing schedule rows.
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

    let mail_timers = crate::mail_timer::arm_missing(ctx);

    let auras = ctx.db.game_aura();
    let stale_procs: Vec<crate::Aura> = auras
        .iter()
        .filter(|a| crate::spell::proc::is_proc_kind(a.eff_kind) && a.proc_flags == 0)
        .collect();
    let mut proc_profiles = 0u64;
    for mut a in stale_procs {
        let Some(hdr) = ctx.db.game_spell().spell_id().find(a.spell_id) else {
            continue;
        };
        let profile = crate::spell::proc::freeze_profile(ctx, a.eff_kind, &hdr);
        if profile.flags == 0 {
            continue;
        }
        a.proc_flags = profile.flags;
        a.proc_chance = profile.chance;
        a.proc_ppm = profile.ppm;
        a.proc_ex = profile.proc_ex;
        a.proc_school_mask = profile.school_mask;
        a.proc_family_name = profile.family_name;
        a.proc_family_flags = profile.family_flags;
        a.proc_charges = profile.charges;
        a.proc_icd_ms = profile.icd_ms;
        a.proc_ready_micros = 0;
        a.update(ctx);
        proc_profiles += 1;
    }

    let total = talents
        + createinfo
        + lesser_heal_target
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
        + weather_climate
        + aura_schedule
        + breath_schedule
        + duel_schedule
        + ground_area_schedule
        + motion_schedule
        + pet_care_schedule
        + auction_expiries
        + mail_timers
        + proc_profiles
        + legacy_auction_mail
        + 3;
    crate::import_meta::stamp(ctx, "debug_repair_after_publish", "", "", total);
    log::info!("debug_repair_after_publish: repaired {total} fixture/schedule row(s), including missing Auction expiries and {mail_timers} Mail Timer(s)");
    Ok(())
}
