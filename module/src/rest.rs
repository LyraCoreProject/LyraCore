//! Rest state (196): entering an inn/rest-area sets the RESTED flag — the client draws the zzz icon and
//! turns the XP bar blue (PLAYER_BYTES_2 byte 3) — accrues rested XP LIVE at the full rate while resting,
//! and (via the durable `Character.resting` stamp) makes an OFFLINE logout in a rest area accrue at the
//! full rate vs 1/4 in the open field. Rest-area geometry is a hand-authored fixture (the Lion's Pride
//! Inn) until the AreaTrigger.dbc + tavern-list import lands — the same fixture-first pattern as the
//! graveyards (`world::graveyard::STATIC_CANDIDATES`). Instant logout in a rest area is already the
//! behaviour: the gateway hardcodes `LogoutSpeed::Instant` (there is no 20s field timer to skip).

use spacetimedb::{table, ReducerContext, Table, Timestamp};

use crate::game_character; // accessor trait
#[cfg(feature = "debug_reducers")]
use crate::game_world_entity; // accessor trait (debug_check_rest_at)
use crate::world::WorldEntity;
use lyracore_shared::packing;

/// A rest-area trigger sphere. 2-D radius test (z-blind, like the graveyard fixtures). Deliberate
/// simplification: a flat const list is exact for the fixture slice; swap for an imported
/// `game_rest_trigger` table when the AreaTrigger.dbc tavern list lands (the graveyard
/// `game_graveyard`-seeded precedent).
struct RestTrigger {
    map_id: u32,
    x: f32,
    y: f32,
    radius: f32,
}

const REST_TRIGGERS: &[RestTrigger] = &[
    // Lion's Pride Inn, Goldshire (Elwynn, map 0). Center + a radius covering the interior. FIXTURE
    // ESTIMATE — confirm the exact threshold against the real inn on the live-eyeball pass (196).
    RestTrigger {
        map_id: 0,
        x: -9464.0,
        y: 42.0,
        radius: 20.0,
    },
];

/// Is `(map,x,y)` inside any rest-area trigger? Pure + cheap (a map check + a few float ops over the
/// fixture list) — this is the whole hot-path cost of the per-heartbeat rest check; the DB work below
/// only runs on a threshold crossing.
fn in_rest_area(map_id: u32, x: f32, y: f32) -> bool {
    REST_TRIGGERS.iter().any(|t| {
        t.map_id == map_id && {
            let (dx, dy) = (x - t.x, y - t.y);
            dx * dx + dy * dy <= t.radius * t.radius
        }
    })
}

/// Rest-state descriptor event → the gateway relays PLAYER_BYTES_2 so the client flips the zzz icon +
/// blue XP bar the instant you cross an inn threshold. Public/broadcast (like the explored relay); the
/// gateway filters to self. Carries the FULL `player_bytes_2` because a partial VALUES overwrites the
/// whole field — byte 0 (facial hair) must ride along, only byte 3 (rest state) changes.
#[table(accessor = game_rest_state_event, public)]
pub struct RestStateEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub character_guid: u64,
    pub player_bytes_2: u32,
    pub created_at: Timestamp,
}

// Character-owned sweep: a rest-state event is per-character relay data, deleted with its character.
// Public (no `owner_identity`, no per-owner RLS) → `delete` only, no `restamp`.
crate::character_owned!(delete, fn sweep_delete_game_rest_state_event(ctx, character_guid) {
    let ev = ctx.db.game_rest_state_event();
    for r in ev.iter().filter(|r| r.character_guid == character_guid).collect::<Vec<_>>() {
        ev.id().delete(r.id);
    }
});
// CROSS-DATABASE transport: a rest-state event is a one-shot RELAY row with a GC TTL —
// the gateway turns it into one PLAYER_BYTES_2 packet and it is done. It carries no durable state
// (the durable half is `Character.resting`/`rested_xp`, which rides the character row), so carrying
// it would replay a stale zzz-icon flip at the destination. Exports NOTHING, deliberately: the arm
// exists so the "every manifest table has a transport" ratchet stays a ratchet, and so this decision
// is written down at the table instead of being an omission nobody notices.
crate::character_owned!(not_transported, fn sweep_transfer_game_rest_state_event());

/// Called every movement heartbeat (NOT grid-gated — an inn is smaller than a 50yd grid cell, so a
/// coarse crossing check would miss it). HOT PATH: a cheap `in_rest_area` test vs the already-loaded
/// `mover.resting` — an in-memory compare, zero DB. Only a real CROSSING flips the flag: swaps the
/// PLAYER_BYTES_2 rest byte (mutates `mover`, folded into the caller's single entity update), (re)stamps
/// the durable live-accrual clock, and emits the relay event. `mover.resting` is restored from
/// `Character.resting` at spawn and persisted back at logout, so it survives a relog.
pub(crate) fn check_rest_state(ctx: &ReducerContext, mover: &mut WorldEntity) {
    let now_resting = in_rest_area(mover.map_id, mover.x, mover.y);
    if now_resting == mover.resting {
        return; // no crossing — the common case, no DB touched
    }
    mover.resting = now_resting; // in-memory; folded into the caller's `entities.guid().update(mover)`

    let now = ctx.timestamp.to_micros_since_unix_epoch();
    let chars = ctx.db.game_character();
    if let Some(mut c) = chars.guid().find(mover.guid) {
        c.resting = now_resting;
        if now_resting {
            c.rested_since_micros = now as u64; // start the live-accrual clock
        } else {
            // Leaving: bank whatever accrued this stay, then stop the clock.
            let elapsed = (now as u64).saturating_sub(c.rested_since_micros);
            c.rested_xp = crate::xp::rest_accrue_live(c.rested_xp, elapsed, c.level as u32);
            c.rested_since_micros = 0;
        }
        chars.guid().update(c);
    }

    // Swap byte 3 (rest state), keep byte 0 (facial hair) + bytes 1-2. Fold into the caller's update.
    let rest_byte = if now_resting {
        packing::REST_STATE_RESTED
    } else {
        packing::REST_STATE_NORMAL
    } as u32;
    mover.player_bytes_2 = (mover.player_bytes_2 & 0x00FF_FFFF) | (rest_byte << 24);

    ctx.db.game_rest_state_event().insert(RestStateEvent {
        id: 0,
        character_guid: mover.guid,
        player_bytes_2: mover.player_bytes_2,
        created_at: ctx.timestamp,
    });
}

/// Bank the live rested accrual onto a character at logout and stop the clock (the offline path takes
/// over from `last_logout_micros`). Called by `persist_entity` on a real logout so no online accrual is
/// lost before the offline span starts. No-op when the clock is already stopped (`rested_since == 0`).
pub(crate) fn materialize_on_logout(c: &mut crate::character::Character, now_micros: i64) {
    if c.rested_since_micros == 0 {
        return;
    }
    let elapsed = (now_micros as u64).saturating_sub(c.rested_since_micros);
    c.rested_xp = crate::xp::rest_accrue_live(c.rested_xp, elapsed, c.level as u32);
    c.rested_since_micros = 0;
}

/// Headless trigger for the rest-state hook at an EXPLICIT position — the machine-test counterpart to
/// the movement_update hook (debug_teleport writes position directly and never calls movement_update, so
/// it never fires the rest check). Sets the entity's position to (map,x,y) and runs `check_rest_state`
/// ATOMICALLY (the whole reducer is one transaction), so a stay-session heartbeat can't race the
/// position. A connected client's on_insert relay fires for the event this emits. [entity]
#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
pub fn debug_check_rest_at(
    ctx: &ReducerContext,
    guid: u64,
    map_id: u32,
    x: f32,
    y: f32,
) -> Result<(), String> {
    let mut e = crate::helpers::live_entity(ctx, guid)?;
    e.map_id = map_id;
    e.x = x;
    e.y = y;
    check_rest_state(ctx, &mut e);
    ctx.db.game_world_entity().guid().update(e);
    Ok(())
}

// Live rested accrual (196): while resting ONLINE, grow the pool at the full rest rate. Lossless —
// materialize from the fixed `rested_since` clock and only advance the clock once the increment banks
// ≥1 XP, so short ticks (where the sub-1-XP increment rounds to 0) don't lose accrual. Self-quantized to
// ~30s: this pass fires every 0.5s `tick_creatures` tick, but rested is ~0.1 XP/min so cadence only sets
// the write frequency, not the amount. `rested_since > 0` scopes the scan to ONLINE resting characters
// (logout stops the clock), so offline chars — which accrue via the login path — are never double-paid.
crate::game_tick_pass!(fn rested_accrue_pass(ctx) {
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    const QUANTUM_MICROS: i64 = 30_000_000;
    const TICK_MICROS: i64 = 500_000;
    if now.rem_euclid(QUANTUM_MICROS) >= TICK_MICROS {
        return; // fire the scan once per ~30s window
    }
    let chars = ctx.db.game_character();
    // Deliberate simplification: full game_character scan every 30s; add a `by_resting` index if
    // the table grows large.
    //
    // REFUSE verdict. This pass is BACKGROUND, not caller-driven, so "the guid came from
    // a live entity" — the argument that fences every other rest write — does not apply: the filter
    // reads the DURABLE row's flags and nothing here ever looks at `game_world_entity`.
    // `begin_transfer` persists with `set_offline: false`, and it is the `set_offline` branch of
    // `persist_entity` that would have called `materialize_on_logout` to stop the clock — so a
    // character escrowed while standing in an inn keeps `resting == true` and a running
    // `rested_since_micros`, and without this gate the pass keeps writing `rested_xp` to a frozen
    // row every 30s for the whole hop (indefinitely, when `recovery` answers `Hold`).
    //
    // Refusing is LOSSLESS here, which is why it is the right verdict rather than a defer: the
    // accrual is materialised from the fixed `rested_since_micros` clock and the clock is left
    // running, so the first pass after the transfer settles banks the entire span in one step.
    let resting: Vec<crate::character::Character> = chars
        .iter()
        .filter(|c| c.resting && c.rested_since_micros > 0)
        .filter(|c| !crate::transfer::is_in_transit(ctx, c.guid))
        .collect();
    for mut c in resting {
        let elapsed = (now as u64).saturating_sub(c.rested_since_micros);
        let grown = crate::xp::rest_accrue_live(c.rested_xp, elapsed, c.level as u32);
        if grown > c.rested_xp {
            c.rested_xp = grown;
            c.rested_since_micros = now as u64; // advance only when we banked ≥1 XP (lossless)
            chars.guid().update(c);
        }
    }
});
