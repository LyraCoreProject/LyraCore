//! Player corpse object + reclaim. On Release Spirit the player becomes a ghost (see
//! `world::repop`) and a `game_corpse` row — the dead body — is left at the death location. The
//! gateway relays the corpse as a CORPSE-type CREATE_OBJECT (and DESTROY on delete); the ghost runs
//! back and `reclaim_corpse` resurrects at 50%. [entity]/[server]

use spacetimedb::{reducer, table, ReducerContext, Table, Timestamp};

use lyracore_shared::packing::unpack4;

use crate::game_world_entity;
use crate::helpers::entity_by_owner;
use crate::spell::game_resurrect_request;

/// HIGHGUID_CORPSE high bits (0xF101): marks a guid as a corpse object for the 5875 client. The
/// corpse guid is `(HIGHGUID_CORPSE << 48) | owner_guid_low`, so one corpse per player at a time.
pub const HIGHGUID_CORPSE: u64 = 0xF101;

/// The deterministic corpse guid for a player (one corpse per player). Shared by `repop` (spawn),
/// `reclaim_corpse` (the client sends this guid), and the relog/logout cleanup so they all agree.
pub(crate) fn corpse_guid_for(owner_guid: u64) -> u64 {
    (HIGHGUID_CORPSE << 48) | (owner_guid & 0x0000_FFFF_FFFF_FFFF)
}

/// Reclaim radius² — (39 yd)² (vanilla `CORPSE_RECLAIM_RADIUS`). CONFIRM the exact value.
const RECLAIM_RADIUS_SQ: f32 = 1521.0;

// ===========================================================================================
//  Reclaim-delay escalation — the behavioural spec this section implements
// ===========================================================================================
//
// Vanilla's corpse-reclaim penalty is publicly documented behaviour: dying repeatedly in a short
// span makes the ghost wait longer each time before it may take its body back. Restated as the
// contract the code below satisfies (it is written FROM this contract, not transcribed from any
// implementation of it):
//
//   * A DEATH STREAK is tracked as a single durable deadline per character — while `now` is before
//     it, the character is "still in the streak"; once it passes, the streak is over.
//   * Each death banks one RUNG of streak credit (5 real-world minutes) onto that deadline, and
//     reads its reclaim delay off the ladder at the rung now standing.
//   * The ladder is 30s → 60s → 120s. Rung 1 is an ordinary, isolated death.
//   * The streak saturates: once it is holding a full ladder's worth of credit, a further death
//     re-pins the deadline instead of extending it, so a death spiral cannot bank an unbounded
//     penalty (and the delay stays at the ladder's top rung).
//   * A death after the deadline has lapsed — and a character's very first death, whose stored
//     deadline is 0 — starts a fresh streak at rung 1, i.e. the plain 30s.
//
// The vocabulary below (streak, rung, banked credit, re-pin) is ours; only the observable numbers
// (30/60/120s, a 5-minute window, a 3-rung ladder) are the vanilla behaviour being matched.

/// The reclaim-delay ladder, by streak rung: how long the ghost waits before it may reclaim.
/// Rung 1 (index 0) is an ordinary death; the last entry is the saturation ceiling.
pub const CORPSE_RECLAIM_DELAY_SECS: [i64; 3] = [30, 60, 120];

/// One rung of streak credit, in micros. Dying banks this much time onto the streak deadline, and
/// the streak is over once the deadline passes — so 5 quiet real-world minutes clear one rung.
const STREAK_RUNG_MICROS: i64 = 300 * 1_000_000; // 5 min

/// The most credit a streak may hold — derived from the ladder, so the two can never disagree:
/// once the streak is this deep, further deaths re-pin rather than extend it.
const MAX_STREAK_RUNGS: i64 = CORPSE_RECLAIM_DELAY_SECS.len() as i64;

/// How long an unreclaimed corpse (still lootable-by-owner-in-spirit / a reclaim target) sits before
/// decaying to bones (cosmetic remains, no longer reclaimable). UNVERIFIED: vanilla's exact
/// corpse-decay tuning isn't pinned by any source reachable from here — this is OUR chosen value
/// (5 min), not a confirmed cmangos/vanilla constant.
pub(crate) const CORPSE_DECAY_MICROS: i64 = 300 * 1_000_000;

/// How long bones then linger before despawning entirely. Same caveat as `CORPSE_DECAY_MICROS`:
/// this is our chosen value (5 min), not a verified vanilla constant.
pub(crate) const BONES_DECAY_MICROS: i64 = 300 * 1_000_000;

/// Bank this death onto the character's streak and read off the reclaim delay it earns — the whole
/// of the spec above, in three lines of accounting.
///
/// Takes and returns the streak deadline stored on the player row
/// (`WorldEntity::death_expire_micros`; 0 for a character that has never died, which reads as
/// "lapsed" and so starts at rung 1). Returns `(streak_deadline_micros, delay_micros)`: the new
/// deadline to stamp back for the NEXT death to escalate from, and the delay stamped onto the
/// freshly-inserted `Corpse` row (`reclaim_delay_micros`), which the gateway reports to the client
/// via `SMSG_CORPSE_RECLAIM_DELAY`.
pub(crate) fn escalated_reclaim(streak_deadline_micros: i64, now_micros: i64) -> (i64, i64) {
    let credit_remaining = streak_deadline_micros - now_micros;
    let deadline = if credit_remaining <= 0 {
        // Lapsed (or never died): a fresh streak, one rung deep.
        now_micros + STREAK_RUNG_MICROS
    } else if credit_remaining / STREAK_RUNG_MICROS < MAX_STREAK_RUNGS {
        // Still climbing: bank one more rung on top of what is already standing.
        streak_deadline_micros + STREAK_RUNG_MICROS
    } else {
        // Saturated: re-pin a full ladder ahead rather than growing without bound.
        now_micros + MAX_STREAK_RUNGS * STREAK_RUNG_MICROS
    };
    // The rung the ladder is read at = the credit now standing, rounded UP (a part-spent rung still
    // counts as that rung) and clipped to the ladder's ends.
    let rung = ((deadline - now_micros + STREAK_RUNG_MICROS - 1) / STREAK_RUNG_MICROS)
        .clamp(1, MAX_STREAK_RUNGS);
    (
        deadline,
        CORPSE_RECLAIM_DELAY_SECS[rung as usize - 1] * 1_000_000,
    )
}

/// Repack a player's appearance into the `CORPSE_FIELD_BYTES_1/2` layout. This layout is
/// DIFFERENT from `PLAYER_BYTES`: `BYTES_1 = 0 | race<<8 | gender<<16 | skin<<24`,
/// `BYTES_2 = face | hairstyle<<8 | haircolor<<16 | facialhair<<24`. race/gender come from
/// `unit_bytes_0` (bytes 0/2); skin/face/hair from `player_bytes`; facialhair from `player_bytes_2`
/// byte 0. Sending the raw `player_bytes` instead makes the 5875 client read `face` as the race →
/// null-deref the body model → crash (verified). Pure so it's unit-tested without a reducer.
pub(crate) fn corpse_appearance_bytes(
    unit_bytes_0: u32,
    player_bytes: u32,
    player_bytes_2: u32,
) -> (u32, u32) {
    // `unpack4` returns the four little-endian bytes (.0 = bits 0-7 … .3 = 24-31); take the same
    // bytes the inline mask/shifts did. race/gender from `unit_bytes_0` bytes 0/2; skin/face/hair
    // from `player_bytes` bytes 0-3; facialhair from `player_bytes_2` byte 0.
    let (race, _, gender, _) = unpack4(unit_bytes_0);
    let (skin, face, hairstyle, haircolor) = unpack4(player_bytes);
    let (facialhair, _, _, _) = unpack4(player_bytes_2);
    let (race, gender, skin) = (race as u32, gender as u32, skin as u32);
    let (face, hairstyle, haircolor, facialhair) = (
        face as u32,
        hairstyle as u32,
        haircolor as u32,
        facialhair as u32,
    );
    let bytes_1 = (race << 8) | (gender << 16) | (skin << 24);
    let bytes_2 = face | (hairstyle << 8) | (haircolor << 16) | (facialhair << 24);
    (bytes_1, bytes_2)
}

/// The dead body left at a player's death location. A CORPSE-type world object: the gateway
/// relays its insert as CREATE_OBJECT and its delete as SMSG_DESTROY_OBJECT (like a creature). [entity]
#[table(accessor = game_corpse, public)]
pub struct Corpse {
    #[primary_key]
    pub guid: u64, // HIGHGUID_CORPSE in the high bits
    pub owner_guid: u64,
    pub map_id: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub orientation: f32,
    pub display_id: u32, // the player's body model, so the corpse renders as them
    pub bytes_1: u32,    // CORPSE_FIELD_BYTES_1 (race/sex/skin) — from the player's player_bytes
    pub bytes_2: u32,    // CORPSE_FIELD_BYTES_2 (face/hair) — from player_bytes_2
    pub created_at: Timestamp, // for the reclaim delay + corpse decay-to-bones timer

    // The reclaim delay stamped by `escalated_reclaim` at insert time (repeated quick deaths climb
    // the 30s/60s/120s ladder), superseding the old flat 30s comparison in `reclaim_corpse`.
    // `#[default(30_000_000i64)]` (typed — an i64 column needs an explicitly-typed
    // literal, same as the u64 fields elsewhere: a bare untyped literal encodes 4 bytes, publish
    // needs 8) + end-appended so `publish` auto-migrates existing rows (the migration rule:
    // column-add needs a default annotation AND end-append).
    #[default(30_000_000i64)]
    pub reclaim_delay_micros: i64,

    // Body → bones state flip (the gc.rs reaper sets this once `CORPSE_DECAY_MICROS` elapses
    // unreclaimed); bones are cosmetic remains — no longer a reclaim target — and reap on their own
    // timer (`BONES_DECAY_MICROS` after the bones flip). `#[default(false)]` + end-appended so
    // `publish` auto-migrates existing rows.
    #[default(false)]
    pub is_bones: bool,

    // Which instance the death happened in (work-item 190 slice 2): stamped from the dying
    // player's own entity in `do_repop`; 0 = open world (every existing row auto-migrates to 0).
    // Gates reclaim (below) and the gateway's corpse CREATE relay by viewer instance; the instance
    // reap deletes any corpse left inside (the ghost's outcome is then spirit-healer-only —
    // vanilla's expired-corpse rule). END-appended + `#[default(0u64)]` (danger-zones §2).
    // GATEWAY-SUBSCRIBED table → `gateway/src/stdb/bindings/corpse_type.rs` + the
    // `schema_parity.rs` manifest hand-synced in the SAME change (playbook failure-mode #1).
    #[default(0u64)]
    pub instance_id: u64,
}

/// Resurrect the caller at their corpse (`CMSG_RECLAIM_CORPSE`). Validates the caller is a
/// ghost that OWNS this corpse, is on the same map within reclaim range, and the 30s delay elapsed;
/// then restores 50% health, clears the ghost state (health > 0 + cleared flags replicate → the
/// client comes alive), and deletes the corpse (→ SMSG_DESTROY_OBJECT). Authorized via `ctx.sender`.
#[reducer]
pub fn reclaim_corpse(ctx: &ReducerContext, _corpse_guid: u64) -> Result<(), String> {
    use lyracore_shared::constants::{player_flags, unit_vis_flags};
    let entities = ctx.db.game_world_entity();
    let corpses = ctx.db.game_corpse();

    let mut player =
        entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "caller not in world".to_string())?;
    if !player.dead || player.player_flags & player_flags::GHOST == 0 {
        return Err("caller is not a ghost".to_string());
    }
    // Resolve the corpse from the CALLER, never the packet guid. In vanilla 1.12 the client fills
    // CMSG_RECLAIM_CORPSE with its own PLAYER guid (bare low guid), not the 0xF101 corpse guid — so a
    // `find(&packet_guid)` always misses and the reclaim silently fails (this bricked the death loop).
    // There is exactly one corpse per player (`corpse_guid_for`), so derive it; `_corpse_guid` is ignored
    // (kept as a self-sanity arg for wire-shape compatibility). This makes the owner check redundant.
    let corpse_guid = corpse_guid_for(player.guid);
    let corpse = corpses
        .guid()
        .find(corpse_guid)
        .ok_or_else(|| "no such corpse".to_string())?;
    // Map + instance gated (190 slice 2 — corpse rows carry `instance_id` now): a ghost must
    // corpse-run back into the SAME instance it died in (the areatrigger resolve re-binds it to
    // that instance, so the run-back lands right); a ghost in another party's copy — or in the
    // open world — can never reclaim through the wall.
    if corpse.map_id != player.map_id {
        return Err("corpse on another map".to_string());
    }
    if corpse.instance_id != player.instance_id {
        return Err("corpse in another instance".to_string());
    }
    if corpse.is_bones {
        return Err("corpse has decayed to bones".to_string());
    }
    let (dx, dy, dz) = (
        corpse.x - player.x,
        corpse.y - player.y,
        corpse.z - player.z,
    );
    if dx * dx + dy * dy + dz * dz > RECLAIM_RADIUS_SQ {
        return Err("too far from corpse".to_string());
    }
    let elapsed =
        ctx.timestamp.to_micros_since_unix_epoch() - corpse.created_at.to_micros_since_unix_epoch();
    if elapsed < corpse.reclaim_delay_micros {
        return Err("corpse reclaim delay not elapsed".to_string());
    }

    // Resurrect at 50% (vanilla corpse-reclaim percent). Clearing dead + the GHOST flags + restoring
    // health replicates to the client, which leaves the ghost/death state (no "alive" opcode exists).
    player.health = (player.max_health / 2).max(1);
    player.dead = false;
    player.player_flags &= !player_flags::GHOST;
    player.unit_bytes_1 &= !unit_vis_flags::GHOST;
    let player_guid = player.guid;
    entities.guid().update(player);
    corpses.guid().delete(corpse_guid);

    // Reclaiming resolves the death outside of accepting a pending resurrect offer — drop any
    // outstanding `game_resurrect_request` for this player so a stale offer doesn't resurface as a
    // phantom SMSG_RESURRECT_REQUEST on a future reconnect. Idempotent (no-op if none pending).
    ctx.db
        .game_resurrect_request()
        .target_guid()
        .delete(player_guid);
    Ok(())
}

/// Player corpses: an unreclaimed body decays to bones (cosmetic remains, no longer a reclaim
/// target — `reclaim_corpse` above rejects `is_bones` rows) after `CORPSE_DECAY_MICROS`, then the
/// bones themselves despawn after `BONES_DECAY_MICROS` more. Keyed off the corpse's OWN
/// `created_at` — independent of the ghost's reclaim-escalation deadline
/// (`WorldEntity::death_expire_micros`), which only governs how long the NEXT death's delay is, not
/// this body's decay. The state flip is an in-place UPDATE (coords/appearance kept) so the
/// gateway's `on_update` relay can re-emit the CREATE with the bones flag; the despawn is a plain
/// delete (→ SMSG_DESTROY_OBJECT), same as reclaim.
///
/// Called from `gc.rs`'s `reap_movement_events` tick — the same sibling-sweep pattern as
/// `spell::stacking::sweep_dr_state` / `loot::sweep_loot_rolls` (#379 pulled the inline block out of
/// `gc.rs`, which had no business knowing what bones are).
///
/// Full scan is safe: the corpse table holds at most one row per RECENTLY-dead player (reclaim
/// deletes; this pass despawns the rest) — it stays tiny by construction.
pub(crate) fn sweep_corpse_decay(ctx: &ReducerContext) {
    let t = ctx.db.game_corpse();
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    let to_bones: Vec<u64> = t
        .iter()
        .filter(|c| {
            !c.is_bones && now - c.created_at.to_micros_since_unix_epoch() >= CORPSE_DECAY_MICROS
        })
        .map(|c| c.guid)
        .collect();
    for guid in to_bones {
        if let Some(mut c) = t.guid().find(guid) {
            c.is_bones = true;
            t.guid().update(c);
        }
    }
    let bones_gone_micros = CORPSE_DECAY_MICROS + BONES_DECAY_MICROS;
    let to_despawn: Vec<u64> = t
        .iter()
        .filter(|c| {
            c.is_bones && now - c.created_at.to_micros_since_unix_epoch() >= bones_gone_micros
        })
        .map(|c| c.guid)
        .collect();
    for guid in to_despawn {
        t.guid().delete(guid);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        corpse_appearance_bytes, corpse_guid_for, escalated_reclaim, HIGHGUID_CORPSE,
        MAX_STREAK_RUNGS, STREAK_RUNG_MICROS,
    };

    #[test]
    fn corpse_guid_puts_highguid_in_the_top_16_and_keeps_the_owner_low_48() {
        // HIGHGUID_CORPSE lands in bits 48-63; the owner guid's low 48 bits survive verbatim, so
        // the owner is recoverable from the corpse guid (and one owner → one deterministic corpse).
        assert_eq!(corpse_guid_for(42), 0xF101_0000_0000_002A);
        assert_eq!(corpse_guid_for(42) >> 48, HIGHGUID_CORPSE);
        assert_eq!(corpse_guid_for(42) & 0x0000_FFFF_FFFF_FFFF, 42);
        // An owner guid carrying its own high bits (a player HIGHGUID) has them MASKED off — only
        // the low 48 flow into the corpse guid, never a mangled top word.
        let owner = (0xABCD_u64 << 48) | 7;
        assert_eq!(corpse_guid_for(owner), 0xF101_0000_0000_0007);
        // The full low-48 range is preserved (no accidental narrower mask).
        assert_eq!(
            corpse_guid_for(0x0000_FFFF_FFFF_FFFF),
            (HIGHGUID_CORPSE << 48) | 0x0000_FFFF_FFFF_FFFF
        );
    }

    #[test]
    // `(0 << 16)` is the GENDER slot of the packed `unit_bytes_0` layout, spelled out so all four
    // bytes of the fixture line up with the comment below it. Folding it away would hide the very
    // slot this test exists to pin (the crash was race landing in the wrong byte).
    #[allow(clippy::identity_op)]
    fn corpse_bytes_put_race_gender_in_the_right_slots() {
        // Human(race 1)/Male(gender 0): unit_bytes_0 = race | class<<8 | gender<<16 | power<<24.
        let unit_bytes_0 = 1 | (1 << 8) | (0 << 16) | (1 << 24);
        let player_bytes = 5 | (6 << 8) | (7 << 16) | (8 << 24); // skin5 face6 hairstyle7 haircolor8
        let player_bytes_2 = 9; // facialhair9
        let (b1, b2) = corpse_appearance_bytes(unit_bytes_0, player_bytes, player_bytes_2);
        // BYTES_1: byte0=0, byte1=race, byte2=gender, byte3=skin (the crash was race not landing here).
        assert_eq!(b1 & 0xFF, 0);
        assert_eq!((b1 >> 8) & 0xFF, 1, "byte1 must be race");
        assert_eq!((b1 >> 16) & 0xFF, 0, "byte2 must be gender");
        assert_eq!((b1 >> 24) & 0xFF, 5, "byte3 must be skin");
        // BYTES_2: face, hairstyle, haircolor, facialhair.
        assert_eq!(b2 & 0xFF, 6);
        assert_eq!((b2 >> 8) & 0xFF, 7);
        assert_eq!((b2 >> 16) & 0xFF, 8);
        assert_eq!((b2 >> 24) & 0xFF, 9);
    }

    #[test]
    fn escalated_reclaim_climbs_the_ladder_then_caps_then_resets_past_expiry() {
        let now: i64 = 1_800_000_000_000_000; // an arbitrary "real" epoch micros
        let step = STREAK_RUNG_MICROS;

        // First death ever: `death_expire_micros` starts at 0, so `now >= expire` — the reset
        // branch — and the base 30s delay applies.
        let (expire1, delay1) = escalated_reclaim(0, now);
        assert_eq!(delay1, 30_000_000, "first death is the 30s base");
        assert_eq!(expire1, now + step);

        // Second death moments later, still well inside the first escalation window: steps up to 60s.
        let now2 = now + 1_000_000; // 1s later
        let (expire2, delay2) = escalated_reclaim(expire1, now2);
        assert_eq!(delay2, 60_000_000, "second quick death escalates to 60s");

        // Third death, again inside the (now much larger) window: steps up to the 120s cap.
        let now3 = now2 + 1_000_000;
        let (expire3, delay3) = escalated_reclaim(expire2, now3);
        assert_eq!(
            delay3, 120_000_000,
            "third quick death reaches the 120s cap"
        );

        // Fourth quick death: stays pinned at the cap, never exceeds it.
        let now4 = now3 + 1_000_000;
        let (_expire4, delay4) = escalated_reclaim(expire3, now4);
        assert_eq!(
            delay4, 120_000_000,
            "a fourth quick death stays capped at 120s, not beyond"
        );

        // A death after the escalation window has fully lapsed resets to the 30s base, regardless
        // of how high the ladder had climbed.
        let now_later = expire3 + step + 1;
        let (expire_reset, delay_reset) = escalated_reclaim(expire3, now_later);
        assert_eq!(
            delay_reset, 30_000_000,
            "decay past expiry resets to the 30s base"
        );
        assert_eq!(expire_reset, now_later + step);
    }

    /// SATURATION + TOTALITY, across a long death spiral: every death's delay is a real ladder
    /// value, never past the ceiling; the banked deadline always sits ahead of `now` and never runs
    /// away (it is bounded by one rung past a full ladder, which is the re-pin invariant); and the
    /// streak really does clear — a gap of more than the whole ladder is back to the 30s base.
    #[test]
    fn escalated_reclaim_saturates_and_never_leaves_the_ladder() {
        let step = STREAK_RUNG_MICROS;
        let ladder_top =
            super::CORPSE_RECLAIM_DELAY_SECS[super::CORPSE_RECLAIM_DELAY_SECS.len() - 1];
        let mut deadline = 0i64;
        let mut t: i64 = 1_800_000_000_000_000;
        for i in 0..500i64 {
            t += (i * 37_000_000) % (step * 2); // death gaps from instant to two rungs
            let (next, delay) = escalated_reclaim(deadline, t);
            assert!(
                super::CORPSE_RECLAIM_DELAY_SECS.contains(&(delay / 1_000_000)),
                "death {i}: delay {delay} is not a ladder value"
            );
            assert!(
                delay <= ladder_top * 1_000_000,
                "death {i}: delay ran past the ceiling"
            );
            assert!(
                next > t,
                "death {i}: the streak deadline must sit ahead of the death"
            );
            assert!(
                next - t <= (MAX_STREAK_RUNGS + 1) * step,
                "death {i}: the deadline grew past the re-pin bound"
            );
            deadline = next;
        }
        // A quiet spell longer than the whole ladder clears the streak completely.
        let (_, after_a_long_quiet) = escalated_reclaim(deadline, deadline + 1);
        assert_eq!(
            after_a_long_quiet, 30_000_000,
            "a lapsed streak is back to the base delay"
        );
    }
}
