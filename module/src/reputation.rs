//! Per-player reputation standing. Mirrors [`crate::spell::spellbook`]'s `game_player_spell`: a
//! per-player, owner-scoped, durable store with one row per (character, faction). `grant_reputation` is
//! the single mutation path — quest turn-ins call it today; creature-kill rep is a later slice (vanilla
//! kill-rep needs per-creature `RewRep` data we don't import yet). Only player-facing factions
//! (`game_faction.reputation_index != -1`) get a row; the gateway relays `SMSG_SET_FACTION_STANDING` on
//! insert/update so the client's reputation bar moves live, and folds stored standings into the login
//! `SMSG_INITIALIZE_FACTIONS` so a relog doesn't reset the rep pane to neutral. [entity]

use spacetimedb::{table, Identity, ReducerContext, Table};

use crate::{game_character, game_faction, game_faction_template, game_world_entity};

/// A character's current standing with one faction. Per-player, owner-scoped (RLS like
/// `game_player_spell`). Logical key `(character_guid, faction_id)` via an `#[auto_inc]` PK +
/// `by_character` btree. Durable (standing persists across logout). `standing` is the raw reputation
/// value clamped to vanilla's [`REP_MIN`, `REP_MAX`]; the client maps it to Hated..Exalted itself. [entity]
#[table(accessor = game_player_reputation, public, index(accessor = by_character, btree(columns = [character_guid])))]
pub struct PlayerReputation {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub character_guid: u64,
    pub owner_identity: Identity,
    pub faction_id: u32,
    pub standing: i32,
    // END-APPENDED (#[default(0)] → additive auto-migrate; negative defaults aren't accepted by the macro).
    // The Faction.dbc ReputationListID — the SMALL 0..63 index the 5875 client's SMSG_SET_FACTION_STANDING
    // addresses in its rep array. The client does NOT key on faction_id: sending faction_id 72 (Stormwind)
    // instead of its rep-index 19 indexes PAST the 64-slot array → null deref → ERROR #132 crash on the first
    // rep-granting turn-in (Marshal McBride). We STORE it here (grant_reputation has the faction lookup) so the
    // gateway relay sends the right index without joining game_faction (which it doesn't bind). grant_reputation
    // only ever writes rows for rep-bar factions and ALWAYS stamps the real index, so a row never carries the
    // 0 filler in practice; the 4 pre-migration rows are backfilled at deploy.
    #[default(0)]
    pub reputation_index: i32,
    // END-APPENDED (195 slice B): the player checked "At War" for this faction in the rep pane
    // (CMSG_SET_FACTION_ATWAR). Persisted so the checkbox survives relog (folded into the login
    // SMSG_INITIALIZE_FACTIONS flag byte); the gateway's interaction-reaction gate treats an
    // at-war faction's NPCs as hostile. Defaulted bool → additive auto-migrate.
    #[default(false)]
    pub at_war: bool,
}

// Character-owned sweep: reputation is durable per-character data, deleted along with its
// character (mirrors the item/spell/skill/talent/quest sweeps).
crate::character_owned!(delete, fn sweep_delete_game_player_reputation(ctx, character_guid) {
    let reps = ctx.db.game_player_reputation();
    for r in reps.by_character().filter(&character_guid).collect::<Vec<_>>() {
        reps.id().delete(r.id);
    }
});
// CROSS-DATABASE transport (issue #19): standings are durable progression, and the login
// SMSG_INITIALIZE_FACTIONS is built from them — a character arriving without them shows every
// faction at neutral. `id` is a surrogate PK, re-minted.
crate::character_owned!(transfer, fn sweep_transfer_game_player_reputation(ctx, character_guid, io) {
    table = game_player_reputation,
    by = by_character,
    remint = id,
});

// Restamp sweep: reputation carries `owner_identity` + an RLS filter like items/spells/skills/
// talents/quests, so it needs the same re-owning on a gateway-restart relog or the rows go
// RLS-invisible to their own player. No-op when the identity already matches (mirrors
// sweep_restamp_game_item_instance).
crate::character_owned!(restamp, fn sweep_restamp_game_player_reputation(ctx, character_guid, identity) {
    let reps = ctx.db.game_player_reputation();
    for mut r in reps.by_character().filter(&character_guid).collect::<Vec<_>>() {
        if r.owner_identity != identity {
            r.owner_identity = identity;
            reps.id().update(r);
        }
    }
});

/// vanilla reputation floor/ceiling: Hated (−42000) .. Exalted (+42000). The standing is clamped here;
/// the client renders the band (Hated/Hostile/.../Exalted) from the raw value, so we store raw points.
const REP_MIN: i32 = -42000;
const REP_MAX: i32 = 42000;

/// Reputation RANK (vanilla `ReputationRank`, 0=Hated .. 7=Exalted, Neutral=3) for a raw standing.
/// Thresholds are the raw lower bounds mangos uses. Pure — unit-tested. `pub(crate)` so vendor pricing
/// (195) can read it. Neutral is the baseline (an unknown faction → 0 standing → Neutral).
pub(crate) fn reputation_rank(standing: i32) -> u8 {
    match standing {
        s if s >= 42000 => 7, // Exalted
        s if s >= 21000 => 6, // Revered
        s if s >= 9000 => 5,  // Honored
        s if s >= 3000 => 4,  // Friendly
        s if s >= 0 => 3,     // Neutral
        s if s >= -3000 => 2, // Unfriendly
        s if s >= -6000 => 1, // Hostile
        _ => 0,               // Hated
    }
}

/// Vendor buy-price discount PERCENT for a raw standing (195). Vanilla `GetReputationPriceDiscount`:
/// 5% per rank ABOVE Neutral — Friendly 5, Honored 10, Revered 15, Exalted 20; Neutral and below give 0.
/// Buy only (sell is unchanged). Pure — unit-tested.
pub(crate) fn reputation_discount_pct(standing: i32) -> u32 {
    const NEUTRAL: u8 = 3;
    let rank = reputation_rank(standing);
    if rank > NEUTRAL {
        5 * (rank - NEUTRAL) as u32
    } else {
        0
    }
}

/// The vendor buy-price discount PERCENT `player_guid` gets at a creature whose FactionTemplate is
/// `faction_template_id` (195). Resolves the template → its parent Faction.dbc id → the player's standing
/// with that faction → `reputation_discount_pct`. 0 when the vendor has no parent faction, the faction
/// has no rep bar, or the player has no standing row (Neutral). Reuses the `grant_reputation` lookup idiom.
pub(crate) fn vendor_discount_pct(
    ctx: &ReducerContext,
    player_guid: u64,
    faction_template_id: u32,
) -> u32 {
    let Some(tmpl) = ctx
        .db
        .game_faction_template()
        .id()
        .find(faction_template_id)
    else {
        return 0;
    };
    if tmpl.faction == 0 {
        return 0; // no parent faction → no reputation to discount on
    }
    let standing = ctx
        .db
        .game_player_reputation()
        .by_character()
        .filter(&player_guid)
        .find(|r| r.faction_id == tmpl.faction)
        .map(|r| r.standing)
        .unwrap_or(0); // no row → Neutral
    reputation_discount_pct(standing)
}

/// Add `amount` reputation (may be negative) for `player_guid` with `faction_id`, clamped to
/// [`REP_MIN`, `REP_MAX`]. Adds to the existing row, or creates one seeded at the faction's `base_standing`
/// (the Human starting value). NO-OP when the faction has no reputation bar
/// (`game_faction.reputation_index == -1` — wildlife/monsters the client can't display), or when the
/// faction / player is unknown. The gateway's `game_player_reputation` relay sends
/// `SMSG_SET_FACTION_STANDING` on the resulting insert/update. Quest turn-in is the only caller today. [entity]
pub(crate) fn grant_reputation(
    ctx: &ReducerContext,
    player_guid: u64,
    faction_id: u32,
    amount: i32,
) {
    // Only player-facing factions (a rep bar) get a row — skip wildlife/monster factions (index -1) and
    // unknown faction ids; the client has nowhere to show them.
    let (base, rep_index) = match ctx.db.game_faction().faction_id().find(faction_id) {
        Some(f) if f.reputation_index >= 0 => (f.base_standing, f.reputation_index),
        _ => return,
    };
    let reps = ctx.db.game_player_reputation();
    if let Some(mut row) = reps
        .by_character()
        .filter(&player_guid)
        .find(|r| r.faction_id == faction_id)
    {
        row.standing = (row.standing.saturating_add(amount)).clamp(REP_MIN, REP_MAX);
        row.reputation_index = rep_index; // re-stamp (heals a stale pre-migration -1 row on the next grant)
        reps.id().update(row);
        return;
    }
    // First gain with this faction → seed at base + amount. owner_identity from the live entity (fallback
    // the durable character row) so the new row is RLS-visible to the player's connection.
    let Some(owner) = ctx
        .db
        .game_world_entity()
        .guid()
        .find(player_guid)
        .map(|e| e.owner_identity)
        .or_else(|| {
            ctx.db
                .game_character()
                .guid()
                .find(player_guid)
                .map(|c| c.owner_identity)
        })
    else {
        return; // unknown player
    };
    reps.insert(PlayerReputation {
        id: 0,
        character_guid: player_guid,
        owner_identity: owner,
        faction_id,
        standing: base.saturating_add(amount).clamp(REP_MIN, REP_MAX),
        reputation_index: rep_index,
        at_war: false,
    });
}

/// The At-War core, actor-explicit (#479): everything [`set_faction_at_war`] does after resolving
/// WHOSE rep pane this is. Takes the row — the seed arm stamps `owner_identity` off it.
pub(crate) fn apply_set_faction_at_war(
    ctx: &ReducerContext,
    player: crate::WorldEntity,
    reputation_index: u32,
    at_war: bool,
) -> Result<(), String> {
    // Reverse map: rep-array slot → faction. The import claims each used slot exactly once.
    let Some(faction) = ctx
        .db
        .game_faction()
        .iter()
        .find(|f| f.reputation_index == reputation_index as i32)
    else {
        return Err(format!("no faction at reputation index {reputation_index}"));
    };
    let reps = ctx.db.game_player_reputation();
    if let Some(mut row) = reps
        .by_character()
        .filter(&player.guid)
        .find(|r| r.faction_id == faction.faction_id)
    {
        row.at_war = at_war;
        row.reputation_index = reputation_index as i32; // heal a stale pre-migration filler in passing
        reps.id().update(row);
        return Ok(());
    }
    reps.insert(PlayerReputation {
        id: 0,
        character_guid: player.guid,
        owner_identity: player.owner_identity,
        faction_id: faction.faction_id,
        standing: faction.base_standing.clamp(REP_MIN, REP_MAX),
        reputation_index: reputation_index as i32,
        at_war,
    });
    Ok(())
}

#[cfg(test)]
mod rep_tests {
    use super::{reputation_discount_pct, reputation_rank};

    #[test]
    fn rank_thresholds_match_vanilla() {
        assert_eq!(reputation_rank(-42000), 0); // Hated (bottom)
        assert_eq!(reputation_rank(-1), 2); // just under Neutral → Unfriendly
        assert_eq!(reputation_rank(0), 3); // Neutral floor
        assert_eq!(reputation_rank(3000), 4); // Friendly floor
        assert_eq!(reputation_rank(9000), 5); // Honored floor
        assert_eq!(reputation_rank(21000), 6); // Revered
        assert_eq!(reputation_rank(42000), 7); // Exalted
    }

    #[test]
    fn discount_is_5pct_per_rank_above_neutral() {
        assert_eq!(reputation_discount_pct(0), 0); // Neutral → full price
        assert_eq!(reputation_discount_pct(-10000), 0); // hostile → no discount (never a surcharge)
        assert_eq!(reputation_discount_pct(3000), 5); // Friendly → 5%
        assert_eq!(reputation_discount_pct(9000), 10); // Honored → 10% (the work-item's headline case)
        assert_eq!(reputation_discount_pct(21000), 15); // Revered → 15%
        assert_eq!(reputation_discount_pct(42000), 20); // Exalted → 20%
    }
}
