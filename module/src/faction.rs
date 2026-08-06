//! Faction relations. The vanilla client gates the attack cursor using its OWN
//! `FactionTemplate.dbc`; the server needs the same data to decide hostility authoritatively (today
//! combat is unconditional — you can `/startattack` a friendly NPC server-side). `game_faction_template`
//! is loaded from the operator's client `FactionTemplate.dbc` by the importer's `--dbc` mode (see
//! `importer/src/dbc.rs`); `compute_hostile` reads the relationship the DBC's own columns encode
//! (explicit enemy/friend slots overriding the coarse group masks) against our flattened schema.
//!
//! SCOPE: this is the DATA + the pure hostility predicate + tests. It is intentionally **not yet
//! wired** into combat attack-initiation — gating combat changes player-facing behavior that needs
//! live client verification, so flipping the gate is a deliberate follow-up once that live-test is
//! done. [static]
//!
//! VERIFIED against the real client's 314 templates (2026-06-18): the human player template (id 1:
//! faction_group Player|Alliance, enemy_group Horde|Monster) is NOT hostile to guards/vendors
//! (Alliance group 11/12) and IS hostile to Defias/Kobolds (Monster group 17/25) — matching what the
//! client's own cursor shows, since the client reads the SAME DBC.
//!
//! WIRING GUIDANCE (for when combat gates on this): "can A auto-attack B" is `compute_hostile(A,B) ||
//! compute_hostile(B,A)` (a unit hostile to me in EITHER direction is a valid auto-attack target);
//! purely-neutral units (e.g. some beast templates with all-zero group masks) need explicit
//! `/startattack`. Full reaction (rep-based con colors, faction standing) additionally needs
//! `Faction.dbc` BaseReputation — a later refinement; `FactionTemplate` hostility is the combat primitive.

use spacetimedb::{table, ReducerContext};

/// One `FactionTemplate.dbc` row, flattened (no Timestamp → SQL-loadable via `spacetime sql`).
/// `faction` is the parent `Faction.dbc` id that OTHER templates match in their `enemies`/`friends`
/// lists; the `*_group` fields are `FactionGroup` bitmasks; `enemy_*`/`friend_*` are up to four
/// explicit `faction` ids each (`[u32; 4]` isn't a `SpacetimeType`, so they're flattened to columns).
#[table(accessor = game_faction_template, public)]
pub struct FactionTemplate {
    #[primary_key]
    pub id: u32,
    pub faction: u32, // parent Faction.dbc id (what others match in their enemy/friend lists)
    pub faction_group: u32, // groups WE belong to (tested against others' enemy/friend group masks)
    pub friend_group: u32, // group mask we are friendly toward
    pub enemy_group: u32, // group mask we are hostile toward
    pub enemy_0: u32,
    pub enemy_1: u32,
    pub enemy_2: u32,
    pub enemy_3: u32,
    pub friend_0: u32,
    pub friend_1: u32,
    pub friend_2: u32,
    pub friend_3: u32,
}

impl FactionTemplate {
    fn enemies(&self) -> [u32; 4] {
        [self.enemy_0, self.enemy_1, self.enemy_2, self.enemy_3]
    }
    fn friends(&self) -> [u32; 4] {
        [self.friend_0, self.friend_1, self.friend_2, self.friend_3]
    }
}

/// One `Faction.dbc` parent faction's reputation metadata (Elwynn faction system). `reputation_index`
/// is the slot in the client's reputation pane (-1 = NOT player-facing, e.g. wildlife like the wolf's
/// faction 29). `base_standing` is the starting reputation a HUMAN character has with this faction
/// (the test race; per-race generalization is a follow-up — we store the primary race-group's base for
/// now). Loaded by `importer --dbc`. Public so the gateway can build a real `SMSG_INITIALIZE_FACTIONS`
/// (the player's actual standings) instead of the all-neutral placeholder. [static]
#[table(accessor = game_faction, public)]
pub struct Faction {
    #[primary_key]
    pub faction_id: u32,
    pub reputation_index: i32, // -1 (== 0xFFFFFFFF in the DBC) when the faction has no reputation bar
    pub base_standing: i32, // starting rep for a Human (raw reputation points; 0 = bottom of Neutral)
}

/// Resolve ONE direction of a `FactionTemplate.dbc` relationship — the shape both `compute_hostile`
/// and `compute_friendly` are instances of, so the precedence rule is written once.
///
/// The row's own columns dictate the precedence: the four explicit `enemy_*` / `friend_*` slots name
/// specific PARENT factions and therefore override the coarse group masks, and a template with no
/// parent faction (`faction == 0`) cannot be named by anyone's list, so for it only the masks apply.
/// `asserting` is the list that PROVES the relation asked about, `vetoing` the opposite list that
/// disproves it, and `mask` the corresponding group mask tested against the other side's
/// `faction_group`.
fn relation_holds(
    asserting: &[u32; 4],
    vetoing: &[u32; 4],
    mask: u32,
    other: &FactionTemplate,
) -> bool {
    if other.faction != 0 {
        if asserting.contains(&other.faction) {
            return true;
        }
        if vetoing.contains(&other.faction) {
            return false;
        }
    }
    (mask & other.faction_group) != 0
}

/// Is `a` hostile to `b`? The enemy direction of the relationship rule: an explicit enemy entry
/// proves it, an explicit friend entry disproves it, otherwise `a`'s enemy-group mask against `b`'s
/// faction group decides.
pub fn compute_hostile(a: &FactionTemplate, b: &FactionTemplate) -> bool {
    relation_holds(&a.enemies(), &a.friends(), a.enemy_group, b)
}

/// Is `a` friendly to `b`? The friend direction of the same rule. Used for the "can I attack" gate —
/// a unit you are NOT friendly to is attackable (hostile AND neutral), only friendly units are
/// protected.
pub fn compute_friendly(a: &FactionTemplate, b: &FactionTemplate) -> bool {
    relation_holds(&a.friends(), &a.enemies(), a.friend_group, b)
}

/// Look up two faction templates by id and decide hostility. Missing data → not hostile (a safe
/// default). (`compute_hostile`/`compute_friendly` are the pure predicates.)
pub fn is_hostile(ctx: &ReducerContext, attacker_faction: u32, target_faction: u32) -> bool {
    let ft = ctx.db.game_faction_template();
    match (ft.id().find(attacker_faction), ft.id().find(target_faction)) {
        (Some(a), Some(b)) => compute_hostile(&a, &b),
        _ => false,
    }
}

/// Does the attacker consider the target FRIENDLY? `combat::start_attack` rejects an attack only when
/// this is true — so hostile (red) AND neutral (yellow, e.g. Elwynn wolves) targets stay attackable,
/// matching vanilla; only friendly (green) units are protected. Missing data → not friendly (so
/// combat is never blocked when the table isn't loaded; the gate is additionally count-guarded).
pub fn is_friendly(ctx: &ReducerContext, attacker_faction: u32, target_faction: u32) -> bool {
    let ft = ctx.db.game_faction_template();
    match (ft.id().find(attacker_faction), ft.id().find(target_faction)) {
        (Some(a), Some(b)) => compute_friendly(&a, &b),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::too_many_arguments)]
    fn ft(
        id: u32,
        faction: u32,
        fg: u32,
        frg: u32,
        eg: u32,
        en: [u32; 4],
        fr: [u32; 4],
    ) -> FactionTemplate {
        FactionTemplate {
            id,
            faction,
            faction_group: fg,
            friend_group: frg,
            enemy_group: eg,
            enemy_0: en[0],
            enemy_1: en[1],
            enemy_2: en[2],
            enemy_3: en[3],
            friend_0: fr[0],
            friend_1: fr[1],
            friend_2: fr[2],
            friend_3: fr[3],
        }
    }

    #[test]
    fn can_attack_neutral_and_hostile_but_not_friendly() {
        // The "can I attack" gate rejects only FRIENDLY targets. Mirrors the live Northshire pairs:
        // player (friend_group Alliance=0x2) vs an Alliance guard = friendly (protected); vs a neutral
        // beast (all-zero groups, e.g. an Elwynn wolf) = NOT friendly (attackable); vs a Monster mob =
        // NOT friendly (attackable). Gating on `hostile` instead of `friendly` would wrongly block
        // neutral mobs, since a neutral target is neither hostile nor friendly.
        let player = ft(1, 1, 0x3, 0x2, 0xC, [0; 4], [0; 4]); // group Player|Alliance, friendly→Alliance
        let guard = ft(11, 72, 0x3, 0x2, 0xC, [0; 4], [0; 4]); // Alliance
        let wolf = ft(32, 29, 0x0, 0x0, 0x0, [0; 4], [0; 4]); // neutral beast (all-zero groups)
        let kobold = ft(25, 25, 0x8, 0x0, 0x0, [0; 4], [0; 4]); // Monster
        assert!(
            compute_friendly(&player, &guard),
            "player is friendly to an Alliance guard"
        );
        assert!(
            !compute_friendly(&player, &wolf),
            "a neutral wolf is NOT friendly → attackable"
        );
        assert!(
            !compute_friendly(&player, &kobold),
            "a Monster kobold is NOT friendly → attackable"
        );
    }

    #[test]
    fn compute_friendly_explicit_lists_override_group_masks() {
        // Explicit FRIEND wins even though the group masks alone would say "not friendly": a's
        // friend_group is 0 (no mask overlap possible), but b's faction is explicitly listed as a friend.
        let a = ft(1, 0, 0, 0x0, 0, [0; 4], [55, 0, 0, 0]);
        let b = ft(2, 55, 0x4, 0, 0, [0; 4], [0; 4]);
        assert!(
            compute_friendly(&a, &b),
            "explicit friend list wins even with a zero/mismatched friend_group mask"
        );

        // Explicit ENEMY wins even though the group masks alone would say "friendly": c's friend_group
        // (0x4) DOES intersect d's faction_group (0x4), but d's faction is explicitly listed as an enemy.
        let c = ft(3, 0, 0, 0x4, 0, [77, 0, 0, 0], [0; 4]);
        let d = ft(4, 77, 0x4, 0, 0, [0; 4], [0; 4]);
        assert!(
            !compute_friendly(&c, &d),
            "explicit enemy list beats a matching friend-group mask"
        );
    }

    #[test]
    fn explicit_enemy_faction_is_hostile() {
        let a = ft(1, 0, 0x2, 0, 0, [59, 0, 0, 0], [0; 4]); // a lists faction 59 as an enemy
        let b = ft(2, 59, 0x1, 0, 0, [0; 4], [0; 4]); // b's parent faction is 59
        assert!(compute_hostile(&a, &b));
    }

    #[test]
    fn explicit_friend_overrides_hostile_group_mask() {
        // a's enemy_group would match b's faction_group (→ hostile by mask), but b is an explicit friend.
        let a = ft(1, 0, 0, 0, 0xFFFF_FFFF, [0; 4], [77, 0, 0, 0]);
        let b = ft(2, 77, 0x1, 0, 0, [0; 4], [0; 4]);
        assert!(
            !compute_hostile(&a, &b),
            "explicit friend must beat the hostile group mask"
        );
    }

    #[test]
    fn group_mask_decides_with_no_explicit_entry() {
        let a = ft(1, 0, 0, 0, 0x4, [0; 4], [0; 4]); // hostile toward group 0x4
        let hostile_b = ft(2, 0, 0x4, 0, 0, [0; 4], [0; 4]); // is in group 0x4
        let neutral_b = ft(3, 0, 0x2, 0, 0, [0; 4], [0; 4]); // group 0x2 — not in a's enemy mask
        assert!(compute_hostile(&a, &hostile_b));
        assert!(!compute_hostile(&a, &neutral_b));
    }

    #[test]
    fn zero_faction_skips_explicit_lists() {
        // b.faction == 0 → a's explicit enemy/friend lists are not consulted; only the masks apply.
        let a = ft(1, 0, 0, 0, 0, [99, 0, 0, 0], [0; 4]); // would list faction 99, but b.faction is 0
        let b = ft(2, 0, 0x1, 0, 0, [0; 4], [0; 4]); // enemy_group 0 → not hostile
        assert!(!compute_hostile(&a, &b));
    }
}
