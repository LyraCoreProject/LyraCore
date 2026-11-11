use spacetimedb::{log, reducer, ReducerContext, Table};

use crate::game_world_entity;
use crate::{game_auction, game_auction_expiry, game_aura, game_spell, AuctionExpiry};

#[reducer]
pub fn debug_repair_after_publish(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::items::discard_legacy_buyback(ctx);
    crate::spell::repair_cast_auras(ctx);
    let guids: Vec<_> = ctx
        .db
        .game_world_entity()
        .by_entry()
        .filter(0u32)
        .filter(|entity| entity.is_player())
        .map(|entity| entity.guid)
        .collect();
    for guid in guids {
        if crate::helpers::acting_entity_by_guid(ctx, guid).is_some() {
            crate::items::equip::repair(ctx, guid);
            crate::spell::recompute_vitals(ctx, guid);
            crate::spell::recompute_sheet(ctx, guid);
        }
    }

    crate::seed::seed_curated_content(ctx);
    crate::items::seed_compatibility_enchantments(ctx);
    let gm_levels = crate::auth::adopt_character_gm_levels(ctx);
    let lesser_heal_target = crate::seed::repair_lesser_heal_target(ctx);
    let weather_climate = crate::weather::seed_weather_weights(ctx);
    let schedules = crate::seed::arm_interval_schedules(ctx);
    let (auction_mail, _unmapped_auction_mail) = crate::auction::repair_legacy_auction_mail(ctx);
    let auction_expiries = arm_missing_auction_expiries(ctx);
    let mail_timers = crate::mail_timer::arm_missing(ctx);
    let proc_profiles = refreeze_proc_profiles(ctx);

    let repaired = gm_levels
        + lesser_heal_target
        + weather_climate
        + schedules
        + auction_mail
        + auction_expiries
        + mail_timers
        + proc_profiles;
    crate::import_meta::stamp(ctx, "debug_repair_after_publish", "", "", repaired);
    log::info!(
        "debug_repair_after_publish: {gm_levels} GM level(s), {lesser_heal_target} Lesser Heal \
         target(s), {weather_climate} weather row(s), {schedules} schedule(s), {auction_mail} \
         Auction mail, {auction_expiries} Auction expiry(ies), {mail_timers} Mail Timer(s), \
         {proc_profiles} Proc profile(s)"
    );
    Ok(())
}

// Preserve one-shot Auction expiries; restore only missing schedule rows.
fn arm_missing_auction_expiries(ctx: &ReducerContext) -> u64 {
    let mut armed = 0;
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
                scheduled_at: crate::time::schedule_time(ctx, auction.expires_at),
                auction_id: auction.id,
            });
            armed += 1;
        }
    }
    armed
}

fn refreeze_proc_profiles(ctx: &ReducerContext) -> u64 {
    let stale_procs: Vec<crate::Aura> = ctx
        .db
        .game_aura()
        .iter()
        .filter(|a| crate::spell::proc::is_proc_kind(a.eff_kind) && a.proc_flags == 0)
        .collect();
    let mut refrozen = 0;
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
        refrozen += 1;
    }
    refrozen
}
