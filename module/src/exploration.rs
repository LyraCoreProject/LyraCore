//! Exploration / discovery XP (work-item 200; the "Discovered: <area>" toast + map fog landed in
//! #41). Entering a subzone for the FIRST time awards discovery XP and sends TWO packets — this is
//! retail parity, not a duplicate (issue #89; see `check_area_exploration`'s doc comment for the
//! cited source): `SMSG_EXPLORATION_EXPERIENCE` (the toast + fog-clear, off the
//! `game_character_explored` insert) and `SMSG_LOG_XPGAIN` (the "+N experience" text, off a non-kill
//! `game_xp_event` insert — the same relay kill/quest XP use). Hooked into `movement_update`
//! (world.rs) on a grid-cell crossing, so it runs ~once per 50 yd, not per heartbeat.

use spacetimedb::{table, ReducerContext, Table};

#[cfg(feature = "debug_reducers")]
use crate::game_world_entity;
use crate::xp::XpEvent;
use crate::{game_area, game_xp_event, WorldEntity};

/// Per-character record of an explored area — the dedup + persistence store so a subzone awards
/// discovery XP exactly ONCE, ever, AND the gateway's source for the client fog. PUBLIC so the gateway
/// can (a) `on_insert`-relay a fresh discovery → `SMSG_EXPLORATION_EXPERIENCE` + the map fog-clear, and
/// (b) read the full set on login to restore the fog bitmask. Keyed by `character_guid` (stable across
/// relogs) + indexed for the dedup + the gateway's per-character scan. `area_id`/`experience` ride along
/// for the discovery packet (the `area_bit` alone drives the fog). [entity]
///
/// NO RLS FILTER, deliberately, and the scaling half of that is now solved on the GATEWAY side (292
/// second pass): the per-player subscription is `... WHERE character_guid = <viewer>` (see
/// `gateway/src/stdb/subscriptions.rs`'s `explored_query`), so a session caches only its own fog rows
/// instead of every character's. That gets the row reduction a `#[client_visibility_filter]` would,
/// without adding an `owner_identity` column (every RLS filter in this module is
/// `<identity col> = :sender`, so adding one means an end-appended identity column + a restamp sweep +
/// a publish/gateway-restart lockstep). The gateway ALSO keeps its `character_guid == self` guard in
/// each `on_insert`: the
/// coordinator's leg subscribes this table globally (owner token, RLS-bypassed anyway), so that guard —
/// not the query — is what keeps one character's fog off another's client.
#[table(accessor = game_character_explored, public, index(accessor = by_character, btree(columns = [character_guid])))]
pub struct CharacterExplored {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub character_guid: u64,
    pub area_bit: i32,
    /// The AreaTable id (game_area.id) — for SMSG_EXPLORATION_EXPERIENCE's "Discovered: <area>".
    #[default(0)]
    pub area_id: u32,
    /// The discovery XP granted (0 for a non-XP area) — the number in the discovery packet.
    #[default(0)]
    pub experience: u32,
}

// Character-owned sweep: exploration/discovery is durable per-character map-fog data, deleted along with
// its character (mirrors the reputation/skill/quest sweeps). Public (no `owner_identity`, no per-owner
// RLS) → `delete` only, no `restamp`.
crate::character_owned!(delete, fn sweep_delete_game_character_explored(ctx, character_guid) {
    let ex = ctx.db.game_character_explored();
    for r in ex.by_character().filter(&character_guid).collect::<Vec<_>>() {
        ex.id().delete(r.id);
    }
});
// CROSS-DATABASE transport (issue #19): the fog-of-war map is durable progression — losing it at a
// shard boundary would re-fog zones and re-pay discovery XP. `id` is a surrogate PK, re-minted.
crate::character_owned!(transfer, fn sweep_transfer_game_character_explored(ctx, character_guid, io) {
    table = game_character_explored,
    by = by_character,
    remint = id,
});

/// On a grid-cell crossing (called from `movement_update`), award discovery XP if `mover` just entered
/// a subzone it has never explored. Mutates `mover` (XP/level) — the caller's single `update` persists
/// it. A graceful no-op when: area data isn't imported, the cell has no area, the area isn't
/// discoverable (`area_bit < 0` or `exploration_level == 0`), the discovery XP rounds to 0 (grey /
/// capped), or the area was already explored. [entity]
///
/// Issue #89: a paying discovery inserts BOTH `game_character_explored` (→ the gateway's
/// `SMSG_EXPLORATION_EXPERIENCE` "Discovered: <area>" toast, carrying `experience`) AND
/// `game_xp_event` (→ `SMSG_LOG_XPGAIN`, the "+N experience" text) — two packets for one grant, not a
/// double-count. This is retail parity, not an artifact of this codebase: confirmed against vmangos
/// (an authentic-protocol vanilla server) — `Player::CheckAreaExploreAndOutdoor` (Player.cpp) calls
/// `GiveXP(xp, nullptr)`, whose body UNCONDITIONALLY calls `SendLogXPGain` (SMSG_LOG_XPGAIN) for
/// every XP grant regardless of source, and only THEN calls `SendExplorationExperience(area, xp)`
/// (SMSG_EXPLORATION_EXPERIENCE). Quest turn-in does the identical two-packet thing
/// (`Player::RewardQuest` calls `GiveXP` then `SendQuestReward`, whose SMSG_QUESTGIVER_QUEST_COMPLETE
/// also carries the xp number) — see `quest.rs`'s `reward_quest`, which mirrors it. XP itself is
/// granted exactly once either way (`grant_xp`, below); only the FEEDBACK is deliberately doubled,
/// matching what a real client receives.
pub(crate) fn check_area_exploration(ctx: &ReducerContext, mover: &mut WorldEntity) {
    let Some(area_id) = crate::terrain::area_id_at(ctx, mover.map_id, mover.x, mover.y) else {
        return;
    };
    let Some(area) = ctx.db.game_area().id().find(area_id) else {
        return; // area table unimported, or an unknown area — skip rather than guess
    };
    if area.area_bit < 0 {
        return;
    }
    // Dedup: already explored this area_bit? (indexed by_character scan — only this char's own rows.)
    let already = ctx
        .db
        .game_character_explored()
        .by_character()
        .filter(&mover.guid)
        .any(|r| r.area_bit == area.area_bit);
    if already {
        return;
    }
    // Discovery XP (0 for a non-discoverable / grey / capped area → recorded but no XP). Computed
    // BEFORE the insert so it rides in the row — the gateway's on_insert reads it for the
    // SMSG_EXPLORATION_EXPERIENCE "Discovered: <area>" toast below.
    let xp = if area.exploration_level > 0 {
        crate::xp::rated_xp(
            ctx,
            crate::xp::explore_xp(area.exploration_level as u32, mover.level),
        )
    } else {
        0
    };
    ctx.db.game_character_explored().insert(CharacterExplored {
        id: 0,
        character_guid: mover.guid,
        area_bit: area.area_bit,
        area_id,
        experience: xp,
    });
    if xp > 0 {
        crate::xp::grant_xp(ctx, mover, xp); // folds the GM .xprate, spends level thresholds
                                             // "+N experience" feedback via the existing SMSG_LOG_XPGAIN relay (non-kill form) — a SECOND,
                                             // deliberately overlapping packet, not a duplicate: retail sends both a discovery toast AND
                                             // SMSG_LOG_XPGAIN for one discovery (issue #89 — see this fn's doc comment for the vmangos
                                             // citation). The map fog-clear and the "Discovered: <area>" toast both ride the
                                             // game_character_explored insert above (the gateway's on_insert sets the PLAYER_EXPLORED_ZONES
                                             // bit and reads `experience` for the toast).
        ctx.db.game_xp_event().insert(XpEvent {
            id: 0,
            recipient_identity: mover.owner_identity,
            killed_guid: 0,
            total_exp: xp,
            created_at: ctx.timestamp,
            is_kill: false,
        });
    }
}

/// Headless trigger for the exploration hook at an EXPLICIT position — the machine-test counterpart to
/// the movement_update hook (debug_teleport writes position directly and does NOT call movement_update,
/// so it never fires exploration). Sets the entity's position to (map,x,y) and runs the check
/// ATOMICALLY inside this reducer, so a concurrent stay-session movement heartbeat can't race the
/// position (the whole reducer is one transaction). Fetch → set position → check → persist. [entity]
#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
pub fn debug_explore_at(
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
    check_area_exploration(ctx, &mut e);
    ctx.db.game_world_entity().guid().update(e);
    Ok(())
}
