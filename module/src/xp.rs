//! XP gain + leveling. A player who lands the killing blow on a creature gains
//! XP (delivered to the killer only via the `game_xp_event` RLS table) and, on crossing the
//! per-level threshold, dings: level++, max-health recalc + full heal, and a `game_levelup_event`
//! the gateway turns into `SMSG_LEVELUP_INFO`. The XP-bar number syncs at create-object (login);
//! the popup + the heal carry the live feedback (see docs/roadmap).

use spacetimedb::{
    log, table, Identity, ReducerContext, Table, Timestamp,
};

use crate::game_character;
use crate::game_config;
use crate::game_world_entity;
use crate::WorldEntity;

// ===========================================================================================
//  XP / level-up event tables [event] — public, RLS-restricted to the recipient
// ===========================================================================================

/// A per-kill XP award, delivered to the killer only → `SMSG_LOG_XPGAIN`. [event]
#[table(accessor = game_xp_event, public, index(accessor = by_recipient, btree(columns = [recipient_identity])))]
pub struct XpEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub recipient_identity: Identity,
    pub killed_guid: u64,
    pub total_exp: u32,
    pub created_at: Timestamp,
    /// true = a KILL award (SMSG_LOG_XPGAIN "from killing X"); false = a non-kill source (exploration
    /// discovery XP) → the bare "You gain N experience". END-appended, #[default(true)] so existing
    /// rows + the kill path are unchanged. [entity]
    #[default(true)]
    pub is_kill: bool,
}

/// A level-up ("ding"), delivered to the player only → `SMSG_LEVELUP_INFO`. [event]
#[table(accessor = game_levelup_event, public, index(accessor = by_recipient, btree(columns = [recipient_identity])))]
pub struct LevelupEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub recipient_identity: Identity,
    pub new_level: u32,
    pub health_gained: u32,
    pub created_at: Timestamp,
    // Per-level deltas for the rest of the SMSG_LEVELUP_INFO popup: mana gain (0 for non-mana
    // classes — rage/focus/energy never grow per level in vanilla) and the five base-attribute gains,
    // computed in grant_xp's ding loop from the same base_attributes_for/max_power_for curves the stat
    // recompute already reads. END-appended + `#[default(0)]` → additive auto-migration. [event]
    #[default(0)]
    pub mana_gained: u32,
    #[default(0)]
    pub strength_gained: u32,
    #[default(0)]
    pub agility_gained: u32,
    #[default(0)]
    pub stamina_gained: u32,
    #[default(0)]
    pub intellect_gained: u32,
    #[default(0)]
    pub spirit_gained: u32,
}

// ===========================================================================================
//  Curve + award
// ===========================================================================================

/// XP required to advance FROM `level` to the next, exact vanilla 1.12 (cmangos `PlayerXPperLevel`).
/// Indexed L1..59 (L60 = cap → 0). Returns 0 for level 0 and any level >= the cap so the ding loop
/// (`grant_xp`) terminates and the rested-pool math (`rest_pool_after`) treats a capped char as
/// "no level to fill". This is the single source of the leveling curve.
pub fn xp_to_next_level(level: u32) -> u32 {
    // XP_PER_LEVEL[i] = XP to go from level (i+1) to (i+2); i.e. index 0 = L1->L2 = 400.
    const XP_PER_LEVEL: [u32; 59] = [
        400, 900, 1400, 2100, 2800, 3600, 4500, 5400, 6500, 7600, 8800, 10100, 11400, 12900, 14400,
        16000, 17700, 19400, 21300, 23200, 25200, 27300, 29400, 31700, 34000, 36400, 38900, 41400,
        44300, 47400, 50800, 54500, 58600, 62800, 67100, 71600, 76100, 80800, 85700, 90700, 95800,
        101000, 106300, 111800, 117500, 123200, 129100, 135100, 141200, 147500, 153900, 160400,
        167100, 173900, 180800, 187900, 195000, 202300, 209800,
    ];
    if level == 0 || level >= 60 {
        return 0; // level 0 / at the cap → no next-level threshold
    }
    XP_PER_LEVEL[(level - 1) as usize]
}

/// Flat placeholder HP curve (60 at L1, +15/level) — the **fallback** for `stats::max_health_for`
/// when the real cmangos `player_classlevelstats`/`player_levelstats` curve (importer P3) isn't
/// loaded. The ding (`award_xp`) and `player_login` call `stats::max_health_for`, which uses this when
/// a row is missing — so login/leveling work identically before an import (L1 = 60 either way).
pub fn max_health_for_level(level: u32) -> u32 {
    60 + 15 * level.saturating_sub(1)
}

/// XP for a `player_level` character killing a `mob_level` creature. Simplified: base `5*mob+45`,
/// gray-clamped to 0 when the mob is >5 levels below the player, +20% when the mob out-levels them.
/// `pub(crate)` so the Drain Soul soul-shard gate (combat/mod.rs) reuses the SAME grey clamp as kill
/// XP — shard eligibility can never drift from XP eligibility.
pub(crate) fn xp_for_kill(mob_level: u32, player_level: u32) -> u32 {
    if mob_level + 5 < player_level {
        return 0;
    }
    let base = mob_level * 5 + 45;
    if mob_level > player_level {
        base * 6 / 5
    } else {
        base
    }
}

/// Vanilla kill-XP multiplier by creature `rank` (cmangos classification: 0=normal, 1=elite,
/// 2=rare-elite, 3=boss/world-boss, 4=rare). Elites (1/2/3) pay 2× — matching vanilla's `if IsElite()
/// xp *= 2`; normal + rare pay 1×. An unknown rank → 1× (never amplify an unrecognized value). Pure.
fn rank_xp_multiplier(rank: u8) -> u32 {
    match rank {
        1..=3 => 2, // elite / rare-elite / world-boss
        _ => 1,     // normal (0), rare (4), unknown
    }
}

/// The realm XP-rate multiplier from the `game_config` singleton (row id 0). Absent → 1.0× (Blizzlike),
/// so an un-tuned realm is byte-identical to before.
fn xp_rate(ctx: &ReducerContext) -> f32 {
    ctx.db
        .game_config()
        .id()
        .find(0)
        .map(|c| c.xp_rate)
        .unwrap_or(1.0)
}

/// The even GROUP SPLIT of a kill-XP award: `base / share_count`, with `share_count` floored
/// at 1 so the solo path (and a buggy 0) is identity. Integer floor — shares never round UP, and a
/// 1-XP kill split two ways vanishes (vanilla-adjacent). Pure — unit-tested.
pub(crate) fn split_kill_xp(base: u32, share_count: u32) -> u32 {
    base / share_count.max(1)
}

/// Scale a raw XP amount by the realm `xp_rate` (rounded). Applied at each SOURCE — kill XP in
/// `award_xp` and quest XP in turn-in — and crucially BEFORE the rested bonus, NOT in `grant_xp`. Doing
/// it before rested means the rested pool drains by the same (rated) amount it pays out, so the rate
/// can't double-dip via the pool. This matches mangos (`GiveXP`: rate the kill XP, then add rested).
pub(crate) fn rated_xp(ctx: &ReducerContext, base: u32) -> u32 {
    (base as f32 * xp_rate(ctx)).round() as u32
}

/// Base XP for a level — the same `level*5+45` magnitude the kill curve uses, standing in for vanilla's
/// per-level `GetBaseXP` table (mirrors `xp_to_next_level` being a documented stand-in). Pure.
pub(crate) fn explore_base_xp(level: u32) -> u32 {
    level.saturating_mul(5).saturating_add(45)
}

/// Half-width, in levels, of the band around an area's own level inside which discovery pays flat.
/// Outside it the award is adjusted in the player's favour (below the band) or tapered away (above).
const EXPLORE_LEVEL_BAND: i32 = 5;

/// Percentage points knocked off the discovery award per level the player stands ABOVE the band —
/// so the award reaches zero about 25 levels over the area, the point where it has gone fully grey.
const EXPLORE_GREY_TAPER_PCT_PER_LEVEL: i32 = 5;

/// What fraction (percent) of an area's discovery award survives, for a player `levels_above` the
/// band's top edge. Clamped both ways, so a wildly over-levelled player gets 0 and never a negative.
fn explore_grey_taper_pct(levels_above_area: i32) -> u32 {
    (100 - (levels_above_area - EXPLORE_LEVEL_BAND) * EXPLORE_GREY_TAPER_PCT_PER_LEVEL)
        .clamp(0, 100) as u32
}

/// Discovery XP for first entry into an area — the award is sized off the AREA's own
/// `exploration_level` (`game_area.exploration_level`), then adjusted for how far the player's level
/// sits from it:
///
///   * more than `EXPLORE_LEVEL_BAND` levels BELOW the area (you wandered somewhere out of your
///     depth): paid off your OWN level plus the band, not the area's — generous but bounded, so a
///     level-10 character sneaking into a level-40 zone cannot bank a level-40 award;
///   * inside the band either way: the area's own base, flat;
///   * more than the band ABOVE it: the grey taper (`explore_grey_taper_pct`).
///
/// A character at the level cap, and any area with no `exploration_level` (not discoverable), pay
/// nothing. Behaviour citation: this is the vanilla discovery-XP shape the mangos-family cores also
/// implement — the same observable award, written here on our own terms. Pure — the caller wraps it
/// in `rated_xp` and grants via `grant_xp` (which folds the GM `.xprate`), same discipline as kills.
pub(crate) fn explore_xp(exploration_level: u32, player_level: u32) -> u32 {
    if player_level >= 60 || exploration_level == 0 {
        return 0;
    }
    let area_award = explore_base_xp(exploration_level);
    match player_level as i32 - exploration_level as i32 {
        under if under < -EXPLORE_LEVEL_BAND => {
            explore_base_xp(player_level + EXPLORE_LEVEL_BAND as u32)
        }
        over if over > EXPLORE_LEVEL_BAND => area_award * explore_grey_taper_pct(over) / 100,
        _ => area_award,
    }
}

// Rested XP. Vanilla accrues ~5% of a level's XP per 8 hours logged out, capped at 1.5 levels,
// and a rested character earns DOUBLE XP per kill until the pool drains. Both rates live here as pure,
// unit-tested functions; the reducers (`award_xp` to spend, `player_login` to accrue) wire them in.
const REST_ACCRUE_PERCENT_PER_8H: u64 = 5; // 5% of a level's XP per 8h offline
const REST_EIGHT_HOURS_MICROS: u64 = 8 * 3600 * 1_000_000; // 28_800_000_000
const REST_CAP_PERMILLE: u64 = 1500; // the rested pool caps at 1.5 levels of XP (1500‰) — the vanilla ceiling

/// The rested-XP bonus for a kill worth `base_xp` with pool `pool`: `(granted, drained)`. Vanilla doubles
/// the kill XP while rested, draining the pool by the bonus amount — so the bonus is `min(base_xp, pool)`,
/// the player gets `base_xp + bonus`, and the pool drops by `bonus`. With an empty pool → `(base_xp, 0)`
/// (no bonus → byte-identical to the pre-rest award). Pure — unit-tested.
fn rest_bonus(base_xp: u32, pool: u32) -> (u32, u32) {
    let bonus = base_xp.min(pool);
    (base_xp + bonus, bonus)
}

/// The rested pool after being offline `offline_micros` at `level`, starting from `pool`: accrues
/// `REST_ACCRUE_PERCENT_PER_8H`% of a level's XP per 8h, capped at 1.5 levels of rested XP. A capped-level
/// (`xp_to_next_level == 0`) or zero offline time leaves the pool unchanged. u64 math throughout so a
/// multi-day offline span can't overflow. Pure — unit-tested.
fn rest_pool_after(pool: u32, offline_micros: u64, level: u32) -> u32 {
    let level_xp = xp_to_next_level(level) as u64;
    if level_xp == 0 || offline_micros == 0 {
        return pool;
    }
    // u128 intermediate: `level_xp * 5 * offline_micros` overflows u64 past ~4 years offline (the
    // `.min(cap)` below would clamp it, but the multiply itself panics in debug). u128 can't overflow
    // for any realistic span, and the post-division result fits u64 comfortably.
    let accrued =
        ((level_xp as u128) * (REST_ACCRUE_PERCENT_PER_8H as u128) * (offline_micros as u128)
            / (100 * REST_EIGHT_HOURS_MICROS as u128)) as u64;
    let cap = level_xp * REST_CAP_PERMILLE / 1000; // 1.5 levels of rested XP (the vanilla ceiling)
    (pool as u64 + accrued).min(cap) as u32
}

/// Live rested accrual (196): the pool after resting ONLINE for `elapsed_micros` at the FULL rest rate,
/// starting from `pool`. Same math as the offline path (`rest_pool_after`) — a rest-area inn accrues at
/// the full rate whether you're logged in or out. Lossless when driven off a fixed `rested_since` clock:
/// the caller advances the clock only once the increment banks ≥1 XP, so sub-1-XP ticks don't round away.
pub(crate) fn rest_accrue_live(pool: u32, elapsed_micros: u64, level: u32) -> u32 {
    rest_pool_after(pool, elapsed_micros, level)
}

/// Accrue rested XP onto a character for being offline since `last_logout_micros` — called by
/// `player_login`. Returns `(new_rested_pool, consume_logout)`: the grown pool, and `true` once the
/// logout stamp has been consumed (so a re-login without an intervening logout can't double-accrue). A
/// never-logged-out character (`last_logout_micros == 0`) accrues nothing. `was_resting` (196) = logged
/// out in a rest area (inn/city) → the FULL rate; the open field accrues at 1/4 (vanilla). The rate
/// scales linearly with offline time before the 1.5-level cap, so quartering the effective offline span
/// is exactly the 1/4 field rate. [entity]
pub(crate) fn accrue_rested_on_login(
    now_micros: i64,
    last_logout_micros: u64,
    pool: u32,
    level: u32,
    was_resting: bool,
) -> (u32, bool) {
    if last_logout_micros == 0 {
        return (pool, false); // first login / already consumed → no accrual
    }
    let offline = (now_micros as u64).saturating_sub(last_logout_micros);
    let effective = if was_resting { offline } else { offline / 4 };
    (rest_pool_after(pool, effective, level), true)
}

/// Award `attacker_guid` the XP for killing `killed_guid` (a `mob_level` creature), applying any
/// level-ups (max-health recalc + full heal) and the rested-XP bonus (double kill XP while the
/// character's rested pool has XP, draining it). Emits a `game_xp_event` (skipped for a gray 0-XP kill —
/// vanilla shows nothing) and one `game_levelup_event` per level gained. No-op if the attacker has left
/// the world. Called from the combat killing-blow branch for player attackers.
pub(crate) fn award_xp(
    ctx: &ReducerContext,
    attacker_guid: u64,
    killed_guid: u64,
    mob_level: u32,
    rank: u8,
    share_count: u32,
) {
    let entities = ctx.db.game_world_entity();
    let Some(mut p) = entities.guid().find(attacker_guid) else {
        return;
    };
    // Elite kills pay 2× (vanilla `if IsElite() xp *= 2`), then the realm xp_rate — applied HERE, before
    // the rested bonus below, so the pool drains by the same rated amount it grants (no double-dip).
    // GROUP SPLIT: the recipient's own level-based award divided evenly by the in-range member
    // count (each member's grey clamp applies to their OWN level; vanilla's sum-of-levels weighting is
    // a documented follow-up). share_count == 1 is byte-identical to the solo path.
    let base = split_kill_xp(
        rated_xp(
            ctx,
            xp_for_kill(mob_level, p.level) * rank_xp_multiplier(rank),
        ),
        share_count,
    );
    if base == 0 {
        return;
    }
    // Rested-XP bonus: read the durable pool off the character (a player entity's guid == its character
    // guid), double the award up to the pool, and drain it. With an empty pool the award is unchanged.
    // Quest XP (grant_xp direct) is deliberately NOT rested — only this kill path applies the bonus,
    // matching vanilla.
    let chars = ctx.db.game_character();
    let award = match chars.guid().find(attacker_guid) {
        Some(mut c) if c.rested_xp > 0 => {
            let (granted, drained) = rest_bonus(base, c.rested_xp);
            c.rested_xp = c.rested_xp.saturating_sub(drained);
            chars.guid().update(c);
            granted
        }
        Some(_) => base, // character present, no rested pool → the plain award
        None => {
            // award_xp is only called for a PLAYER killer, whose entity guid mirrors its character guid —
            // so a missing row is a data/spawn bug, not a normal path. Degrade safely (no bonus) but warn
            // so the inconsistency surfaces instead of hiding.
            log::warn!(
                "award_xp: player entity {attacker_guid} has no character row — rested XP skipped"
            );
            base
        }
    };
    ctx.db.game_xp_event().insert(XpEvent {
        id: 0,
        recipient_identity: p.owner_identity,
        killed_guid,
        total_exp: award,
        created_at: ctx.timestamp,
        is_kill: true,
    });
    grant_xp(ctx, &mut p, award);
    entities.guid().update(p);
}

/// Add `amount` XP to a live player entity `p` and spend it across as many level thresholds as it
/// crosses (the shared "ding loop": level++, max-health/power recompute + full heal, one
/// `game_levelup_event` per level gained). Mutates `p` in place — the CALLER persists it (so a single
/// `update` covers the XP write plus any dings). This is the single home of the leveling math, shared
/// by the kill award ([`award_xp`]) and quest turn-in rewards ([`crate::quest`]) so they can never
/// drift. No-op for `amount == 0`. Does NOT emit a `game_xp_event` (the kill path owns the
/// `SMSG_LOG_XPGAIN` source-guid line; quest XP has no killed unit). [entity]
pub(crate) fn grant_xp(ctx: &ReducerContext, p: &mut WorldEntity, amount: u32) {
    if amount == 0 {
        return;
    }
    // GM playtest `.xprate` (work-item 223): a basis-points multiplier (10000 = 1×) ON TOP of the
    // realm `xp_rate` (`rated_xp`, already folded in by the caller at each SOURCE — kill XP in
    // `award_xp`, quest XP in `quest.rs`). Applied HERE, the one chokepoint both sources share, so a
    // single multiply can never drift between them. u64 math to avoid a u32*u32 overflow. Missing
    // config row ⇒ 10000 (1×) — byte-identical to before this feature existed.
    let amount = ((amount as u64 * crate::gm::xprate_bp(ctx) as u64) / 10_000) as u32;
    if amount == 0 {
        return;
    }
    p.xp += amount;
    // Race/class drive the real per-level curve (importer P3); they live packed in unit_bytes_0
    // (race | class<<8 | gender<<16 | power<<24). max_health_for/max_power_for fall back to the flat
    // placeholder when the curve isn't loaded.
    let race = p.race();
    let class = p.class();
    let mut leveled = false;
    // Ding loop: spend XP across as many thresholds as it crosses (a big award can be 2+ levels).
    while p.next_level_xp > 0 && p.xp >= p.next_level_xp {
        leveled = true;
        p.xp -= p.next_level_xp;
        p.level += 1;
        // Lift every combat (weapon + Defense) skill line's cap to the new level*5: this
        // is the ONLY place the cap moves mid-session (login reconciles pre-existing rows separately).
        // `current` is untouched: the lag between the raised cap and the still-lower `current` IS the
        // skill-up window `raise_skill`/`gain_weapon_skill`/`gain_defense_skill` climb into.
        crate::skill::raise_combat_caps(ctx, p.guid, p.level);
        // Attributes + armor + max health/power for the new level, via the ONE shared writer
        // (`stats::apply_level_stats` — also used by login and a GM level-set, #362) so this can never
        // drift from either. `delta` carries the pre-recompute values for the popup math below —
        // without this the ding loop would otherwise leave STR/AGI/STA/INT/SPI/armor frozen at their
        // pre-ding values until the next relog, even though combat reads these STORED fields directly
        // (effective_strength -> AP, agility -> dodge, armor -> mitigation, spirit -> regen).
        let delta = crate::stats::apply_level_stats(ctx, p, race, class, p.level);
        // Per-stat popup deltas: new-level curve value minus the pre-ding STORED value. saturating
        // because the stored value can exceed the pure curve (e.g. a +stat aura folded in elsewhere) —
        // the popup then just shows 0 for that stat rather than underflowing.
        let strength_gained = p.strength.saturating_sub(delta.old_strength);
        let agility_gained = p.agility.saturating_sub(delta.old_agility);
        let stamina_gained = p.stamina.saturating_sub(delta.old_stamina);
        let intellect_gained = p.intellect.saturating_sub(delta.old_intellect);
        let spirit_gained = p.spirit.saturating_sub(delta.old_spirit);
        let gained = p.max_health.saturating_sub(delta.old_max_health);
        p.health = p.max_health; // full heal on ding — flows to the client via the on_update health relay
                                 // Grow the power pool too (mana scales with level); mana classes refill on ding like health.
                                 // Mana popup delta: only a MANA class's pool grows per level (rage/focus/energy are flat
                                 // in vanilla), so non-mana classes report 0 rather than a meaningless max_power diff.
        let mana_gained = if lyracore_shared::packing::power_type::for_class(class)
            == lyracore_shared::packing::power_type::MANA
        {
            p.max_power.saturating_sub(delta.old_max_power)
        } else {
            0
        };
        if lyracore_shared::packing::power_type::for_class(class)
            == lyracore_shared::packing::power_type::MANA
        {
            p.power = p.max_power;
        }
        p.next_level_xp = xp_to_next_level(p.level);
        ctx.db.game_levelup_event().insert(LevelupEvent {
            id: 0,
            recipient_identity: p.owner_identity,
            new_level: p.level,
            health_gained: gained,
            created_at: ctx.timestamp,
            mana_gained,
            strength_gained,
            agility_gained,
            stamina_gained,
            intellect_gained,
            spirit_gained,
        });
        // Package notify hook: once per level gained. The caller persists the
        // mutated entity row AFTER this loop — handlers read the payload, not the table (documented
        // on `hooks::LevelupPayload`).
        crate::hooks::fire_on_levelup(
            ctx,
            &crate::hooks::LevelupPayload {
                character_guid: p.guid,
                new_level: p.level,
            },
        );
    }
    if leveled {
        // Sheet AP/damage-range are level-derived (#517) and only ever move via `recompute_sheet`,
        // which re-fetches the row by guid — so the ding's level/stat write must be PERSISTED first
        // (a mid-loop call would see the still-stale pre-ding row). Pull the recomputed row back into
        // `p` afterward so the caller's own `entities.guid().update(p)` (documented above as the
        // single post-loop persist) doesn't stomp the fresh `sheet_*` fields with `p`'s stale
        // in-memory copy.
        // `WorldEntity` isn't `Clone`, so swap the up-to-date in-memory struct out of `*p` (via a
        // throwaway placeholder fetched from the table) rather than cloning it, persist that, then
        // read the recomputed row back into `*p`.
        let entities = ctx.db.game_world_entity();
        if let Some(placeholder) = entities.guid().find(p.guid) {
            let moved = std::mem::replace(p, placeholder);
            entities.guid().update(moved);
            crate::spell::recompute_sheet(ctx, p.guid);
            if let Some(fresh) = entities.guid().find(p.guid) {
                *p = fresh;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        accrue_rested_on_login, explore_base_xp, explore_xp, max_health_for_level,
        rank_xp_multiplier, rest_bonus, rest_pool_after, split_kill_xp, xp_for_kill,
        xp_to_next_level, REST_EIGHT_HOURS_MICROS,
    };

    #[test]
    fn explore_xp_scales_off_area_level_and_caps() {
        // In-band (|player - area| <= 5): the area's base = 5*area + 45.
        assert_eq!(explore_xp(10, 10), 95);
        assert_eq!(explore_xp(10, 12), 95); // 2 over, still in-band
                                            // Far-higher area (diff < -5): the player's own (level+5) base.
        assert_eq!(explore_xp(20, 10), explore_base_xp(15)); // 10-20 = -10 < -5
                                                             // Grey (diff > 5): linearly reduced. 6 over → pct = 100-(1*5) = 95%.
        assert_eq!(explore_xp(10, 16), 95 * 95 / 100);
        // 25+ over → 0% (grey).
        assert_eq!(explore_xp(2, 30), 0);
        // Caps: level 60 and a non-discoverable (0-level) area pay nothing.
        assert_eq!(explore_xp(10, 60), 0);
        assert_eq!(explore_xp(0, 10), 0);
    }

    #[test]
    fn kill_xp_pays_base_greys_out_and_adds_20pct_for_an_out_leveling_mob() {
        // Base branch (mob at or below the player's level): 5*mob + 45.
        assert_eq!(xp_for_kill(1, 1), 50);
        assert_eq!(xp_for_kill(10, 10), 95);
        assert_eq!(xp_for_kill(60, 60), 345);
        // GREY clamp boundary: exactly 5 levels below still pays; 6 below is grey (0).
        assert_eq!(xp_for_kill(5, 10), 70); // 5 below → 5*5+45, still pays
        assert_eq!(xp_for_kill(4, 10), 0); // 6 below → grey, no XP
        assert_eq!(xp_for_kill(1, 60), 0); // a L60 farming L1 mobs earns nothing
                                           // +20% branch (mob STRICTLY out-levels the player): base * 6/5, integer math.
        assert_eq!(xp_for_kill(11, 10), 120); // (11*5+45)=100 → 120
        assert_eq!(xp_for_kill(2, 1), 66); // (2*5+45)=55 → 330/5 = 66 exactly
    }

    #[test]
    fn group_split_divides_evenly_and_never_rounds_up() {
        // Solo is identity — share_count 1 leaves the award unchanged.
        assert_eq!(split_kill_xp(100, 1), 100);
        // Even split floors: 100 XP three ways is 33 each, never 34.
        assert_eq!(split_kill_xp(100, 3), 33);
        // A 1-XP kill split two ways vanishes (vanilla-adjacent).
        assert_eq!(split_kill_xp(1, 2), 0);
        // A buggy share_count of 0 degrades to solo, not a divide-by-zero panic.
        assert_eq!(split_kill_xp(100, 0), 100);
        // Full-party split.
        assert_eq!(split_kill_xp(100, 5), 20);
    }

    #[test]
    fn rest_bonus_doubles_until_the_pool_drains() {
        // Full bonus when the pool covers the kill: 50 XP kill, pool 1000 → 100 granted, 50 drained.
        assert_eq!(rest_bonus(50, 1000), (100, 50));
        // Partial bonus when the pool is smaller than the kill: pool 20 → +20 bonus only.
        assert_eq!(rest_bonus(50, 20), (70, 20));
        // Empty pool → no bonus (byte-identical to the pre-rest award), nothing drained.
        assert_eq!(rest_bonus(50, 0), (50, 0));
        // Exactly-covering pool drains to 0.
        assert_eq!(rest_bonus(50, 50), (100, 50));
    }

    #[test]
    fn rest_pool_accrues_5pct_per_8h_capped_at_1_5_levels() {
        // L1 level-XP is 400. 8h offline → 5% of 400 = 20.
        assert_eq!(rest_pool_after(0, REST_EIGHT_HOURS_MICROS, 1), 20);
        // 24h → 60; accrual is linear in offline time.
        assert_eq!(rest_pool_after(0, 3 * REST_EIGHT_HOURS_MICROS, 1), 60);
        // Adds onto an existing pool.
        assert_eq!(rest_pool_after(100, REST_EIGHT_HOURS_MICROS, 1), 120);
        // Capped at 1.5 levels (600 at L1) no matter how long offline.
        assert_eq!(rest_pool_after(0, 10_000 * REST_EIGHT_HOURS_MICROS, 1), 600);
        // Zero offline / capped level (60 → level-XP 0) → unchanged.
        assert_eq!(rest_pool_after(123, 0, 1), 123);
        assert_eq!(rest_pool_after(123, REST_EIGHT_HOURS_MICROS, 60), 123);
    }

    #[test]
    fn elite_rank_pays_double_xp() {
        // Elite / rare-elite / world-boss (rank 1/2/3) → 2×; normal (0), rare (4), unknown → 1×.
        assert_eq!(rank_xp_multiplier(0), 1); // normal
        assert_eq!(rank_xp_multiplier(1), 2); // elite (e.g. Hogger)
        assert_eq!(rank_xp_multiplier(2), 2); // rare-elite
        assert_eq!(rank_xp_multiplier(3), 2); // world boss
        assert_eq!(rank_xp_multiplier(4), 1); // rare — NOT elite
        assert_eq!(rank_xp_multiplier(9), 1); // unknown → never amplify
    }

    #[test]
    fn accrue_on_login_needs_a_prior_logout_and_is_consumed_once() {
        let now = 1_000_000 * 3600 * 24; // arbitrary epoch micros
                                         // Never logged out (stamp 0) → no accrual, stamp not consumed.
        assert_eq!(accrue_rested_on_login(now, 0, 0, 1, true), (0, false));
        // Logged out 8h ago at L1 in a REST AREA → full rate = +20 rested, stamp consumed (true) so a
        // re-login can't re-accrue from the same logout.
        let eight_h_ago = (now as u64) - REST_EIGHT_HOURS_MICROS;
        assert_eq!(
            accrue_rested_on_login(now, eight_h_ago, 0, 1, true),
            (20, true)
        );
        // A clock skew (last_logout in the future) saturates to 0 offline → no accrual, still consumed.
        assert_eq!(
            accrue_rested_on_login(now, (now as u64) + 999, 5, 1, true),
            (5, true)
        );
    }

    #[test]
    fn offline_rest_rate_is_full_in_a_rest_area_and_quarter_in_the_field() {
        let now = 1_000_000 * 3600 * 24;
        let eight_h_ago = (now as u64) - REST_EIGHT_HOURS_MICROS;
        // Logged out in an inn → full rate (+20 at L1). In the open field → 1/4 (+5). Both consumed.
        assert_eq!(
            accrue_rested_on_login(now, eight_h_ago, 0, 1, true),
            (20, true)
        );
        assert_eq!(
            accrue_rested_on_login(now, eight_h_ago, 0, 1, false),
            (5, true)
        );
    }

    #[test]
    fn health_curve_agrees_with_login_and_ding() {
        // Locks the curve so player_login and the ding loop stay in agreement: L1=60, L2=75, L3=90
        // (60 + 15*(level-1)).
        assert_eq!(max_health_for_level(1), 60);
        assert_eq!(max_health_for_level(2), 75);
        assert_eq!(max_health_for_level(3), 90);
    }

    #[test]
    fn xp_curve_matches_vanilla_low_levels_and_caps() {
        assert_eq!(xp_to_next_level(1), 400);
        assert_eq!(xp_to_next_level(2), 900);
        assert_eq!(xp_to_next_level(3), 1400);
        assert_eq!(xp_to_next_level(4), 2100);
        assert_eq!(xp_to_next_level(5), 2800);
        assert_eq!(xp_to_next_level(10), 7600);
        assert_eq!(xp_to_next_level(11), 8800);
        assert_eq!(xp_to_next_level(40), 90700);
        assert_eq!(xp_to_next_level(59), 209800); // last real threshold (cmangos player_xp_for_level)
        assert_eq!(xp_to_next_level(60), 0, "no XP needed at the cap");
        assert_eq!(xp_to_next_level(61), 0, "past the cap stays 0");
        assert_eq!(xp_to_next_level(0), 0);
    }
}
