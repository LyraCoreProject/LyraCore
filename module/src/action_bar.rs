//! Creation-time action-bar rows — `game_player_action` is the per-character bar the gateway's login
//! sequence reads FIRST (before falling back to synthesizing a bar from the spellbook, see
//! `gateway/src/codec/entity.rs::login_sequence_messages`). Mirrors the `game_createinfo_spell` /
//! `game_player_spell` precedent (work-item 178) one level over: `game_createinfo_action` is the
//! per-(race, class) dump-sourced default layout (`playercreateinfo_action`), copied into
//! `game_player_action` at character creation exactly like `grant_createinfo_spells` copies the spell
//! kit. UNLIKE the spell kit there is no hand-authored fallback seeded into `game_createinfo_action` —
//! vanilla's real default bars vary too much per class/level to hand-author cheaply, and the gateway
//! already has a working synth (Attack + known spells) for the pre-import state. So: no import yet →
//! `game_createinfo_action` is empty → creation grants NOTHING → the gateway's existing synth covers
//! login, byte-identical to today. Import lands → creation copies the real rows → the gateway prefers
//! them. [entity]

use spacetimedb::{
    client_visibility_filter, reducer, table, Filter, Identity, ReducerContext, Table,
};

use crate::spell::spellbook::createinfo_row_matches; // shared (race,class) wildcard-match helper

/// One createinfo action-bar button: the vanilla default layout for a fresh character of a matching
/// (race, class). `race == 0` / `class == 0` wildcards are supported via the SAME
/// `createinfo_row_matches` helper `game_createinfo_spell` uses, though the dump is expected to carry
/// concrete (race, class) rows for every real combo (no wildcard convention documented for
/// `playercreateinfo_action` the way there is for `playercreateinfo_spell`) — supporting it costs
/// nothing and keeps the two createinfo tables' matching semantics identical.
/// `action` | `action_type` mirror the client's action-button encoding (`action_type` 0 = spell, other
/// values = item/macro/etc — see `gateway/src/codec/entity.rs`'s packing). Hand-authored data has no
/// client DBC of its own (createinfo exists in no DBC); this table is entirely importer-populated
/// (`playercreateinfo_action`, work-item 212) — never seeded, unlike `game_createinfo_spell`. `row_id`
/// is an importer-assigned dense id (the `game_start_item`/`game_graveyard_zone` convention), not a
/// logical key — the table is a flat bag of rows scanned in full, like `game_createinfo_spell`. [static]
#[table(accessor = game_createinfo_action, public)]
pub struct CreateinfoAction {
    #[primary_key]
    #[auto_inc]
    pub row_id: u64,
    pub race: u8,
    pub class: u8,
    pub button: u8,
    pub action: u32,
    pub action_type: u8,
}

/// A character's live action-bar button — copied from `game_createinfo_action` at creation (see
/// `grant_createinfo_actions`). Per-character, owner-scoped RLS (mirrors `game_player_spell`/
/// `game_player_skill`). Logical key `(character_guid, button)` via an `#[auto_inc]` PK + a
/// `by_character` btree — module-written only (the importer never touches this table; it populates
/// `game_createinfo_action` instead, which creation reads FROM). [entity]
#[table(accessor = game_player_action, public, index(accessor = by_character, btree(columns = [character_guid])))]
pub struct PlayerAction {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub character_guid: u64,
    pub owner_identity: Identity,
    pub button: u8,
    pub action: u32,
    pub action_type: u8,
}

/// A player connection sees only its own action-bar rows (mirrors the item/skill/spell RLS filters).
#[client_visibility_filter]
const PLAYER_ACTION_RLS: Filter =
    Filter::Sql("SELECT * FROM game_player_action WHERE owner_identity = :sender");

// Character-owned sweeps: action-bar rows are deleted on character delete, re-owned (identity
// re-stamp) on a relog under a changed gateway identity — same pattern as `game_player_spell`.
crate::character_owned!(delete, fn sweep_delete_game_player_action(ctx, character_guid) {
    let actions = ctx.db.game_player_action();
    for r in actions.by_character().filter(&character_guid).collect::<Vec<_>>() {
        actions.id().delete(r.id);
    }
});
// CROSS-DATABASE transport (issue #19). `id` is a surrogate `#[auto_inc]` PK — the LOGICAL key is
// (character, button) — so it is re-minted at the destination rather than carried; carrying it would
// insert explicit ids without advancing the destination's sequence (danger-zones §2).
crate::character_owned!(transfer, fn sweep_transfer_game_player_action(ctx, character_guid, io) {
    table = game_player_action,
    by = by_character,
    remint = id,
});
crate::character_owned!(restamp, fn sweep_restamp_game_player_action(ctx, character_guid, identity) {
    let actions = ctx.db.game_player_action();
    for mut r in actions.by_character().filter(&character_guid).collect::<Vec<_>>() {
        if r.owner_identity != identity {
            r.owner_identity = identity;
            actions.id().update(r);
        }
    }
});

/// Pure filter+project step behind `grant_createinfo_actions`, pulled out so it's unit-testable
/// without a live `ReducerContext` (this crate's tests never construct one — see the module test
/// suite's convention of testing only ctx-free helpers). Returns `(button, action, action_type)` for
/// every `game_createinfo_action` row matching (`race`, `class`) via the shared wildcard predicate,
/// AT MOST ONE per button (first match wins — a wildcard row overlapping a concrete row, or a
/// malformed dump with duplicate buttons, must not produce two `game_player_action` rows the gateway
/// would resolve by iteration order). Empty input (pre-import) or no match → empty output — the
/// "create nothing" fallback policy is structurally guaranteed here, not just documented.
pub(crate) fn matching_actions<'a>(
    rows: impl Iterator<Item = &'a CreateinfoAction>,
    race: u8,
    class: u8,
) -> Vec<(u8, u32, u8)> {
    let mut seen_buttons = [false; 256];
    rows.filter(|r| createinfo_row_matches(r.race, r.class, race, class))
        .filter(|r| !std::mem::replace(&mut seen_buttons[r.button as usize], true))
        .map(|r| (r.button, r.action, r.action_type))
        .collect()
}

/// Copy the createinfo action bar for (`race`, `class`) into `guid`'s `game_player_action` rows — the
/// character-creation grant, called right after `grant_createinfo_spells` (same transaction as
/// `create_character`'s insert). A full scan with wildcard matching, like `grant_createinfo_spells`
/// (the table stays tiny — one row per button per class, not per character). If NO row matches (the
/// common case pre-import: `game_createinfo_action` is empty), this inserts NOTHING — deliberately: the
/// gateway's login synth (`login_sequence_messages`) already builds a working bar from the spellbook,
/// and an empty `game_player_action` is exactly the signal it reads as "no imported bar, fall back."
pub(crate) fn grant_createinfo_actions(
    ctx: &ReducerContext,
    guid: u64,
    owner: Identity,
    race: u8,
    class: u8,
) {
    let actions = ctx.db.game_player_action();
    let rows: Vec<CreateinfoAction> = ctx.db.game_createinfo_action().iter().collect();
    for (button, action, action_type) in matching_actions(rows.iter(), race, class) {
        // Idempotent like `learn_spell`: a re-grant (there is none today — creation runs once per
        // guid) must never stack a second row on an occupied button.
        if actions
            .by_character()
            .filter(&guid)
            .any(|a| a.button == button)
        {
            continue;
        }
        actions.insert(PlayerAction {
            id: 0, // auto_inc
            character_guid: guid,
            owner_identity: owner,
            button,
            action,
            action_type,
        });
    }
}

/// Persist one action-bar button change (`CMSG_SET_ACTION_BUTTON` — the client sends one per
/// drag/clear, and expects the server to echo the full bar back at login via SMSG_ACTION_BUTTONS).
/// Upserts by (character, button); `action == 0` is the drag-OFF clear (delete the row). Without
/// this every bar change was LOST on relog — only the creation-seeded buttons survived (user find:
/// a talent-learned Consecration vanished from the bar while staying in the book). `action` is the
/// client's packed u24 (spell id, or item id + misc<<16), `action_type` the high type byte —
/// stored verbatim; the login builder re-packs `action | type << 24`. Owner-authorized.
#[reducer]
pub fn set_action_button(
    ctx: &ReducerContext,
    button: u8,
    action: u32,
    action_type: u8,
) -> Result<(), String> {
    let character = crate::helpers::entity_by_owner(ctx, ctx.sender())
        .ok_or_else(|| "not in world".to_string())?;
    apply_set_action_button(ctx, character, button, action, action_type)
}

/// The action-bar write core, actor-explicit (#479): everything [`set_action_button`] does after
/// resolving WHOSE bar this is. Takes the row — the insert arm stamps the owner's RLS identity
/// off it.
pub(crate) fn apply_set_action_button(
    ctx: &ReducerContext,
    character: crate::WorldEntity,
    button: u8,
    action: u32,
    action_type: u8,
) -> Result<(), String> {
    let actions = ctx.db.game_player_action();
    let existing = actions
        .by_character()
        .filter(&character.guid)
        .find(|r| r.button == button);
    if action == 0 {
        if let Some(r) = existing {
            actions.id().delete(r.id);
        }
        return Ok(());
    }
    match existing {
        Some(mut r) => {
            r.action = action;
            r.action_type = action_type;
            actions.id().update(r);
        }
        None => {
            actions.insert(PlayerAction {
                id: 0,
                character_guid: character.guid,
                // The BAR OWNER's own binding, not `ctx.sender()` — identical on the sender path
                // (the entity was resolved BY that identity) and correct on the gateway path,
                // where `ctx.sender()` is the shared connection's operator identity.
                owner_identity: character.owner_identity,
                button,
                action,
                action_type,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        row_id: u64,
        race: u8,
        class: u8,
        button: u8,
        action: u32,
        action_type: u8,
    ) -> CreateinfoAction {
        CreateinfoAction {
            row_id,
            race,
            class,
            button,
            action,
            action_type,
        }
    }

    /// Confirm the reused `createinfo_row_matches` helper behaves identically for actions as for
    /// spells (it's the SAME function, so this is a byte-parity sanity check, not new logic).
    #[test]
    fn createinfo_row_matches_is_reused_verbatim_for_actions() {
        assert!(createinfo_row_matches(1, 1, 1, 1)); // Human Warrior row on a Human Warrior
        assert!(!createinfo_row_matches(1, 1, 1, 4)); // class mismatch
        assert!(createinfo_row_matches(0, 1, 3, 1)); // race-wildcard class row matches any race
    }

    /// The empty-table case (no import yet) — the load-bearing "create nothing" fallback policy.
    /// `matching_actions` on an empty slice returns empty, so `grant_createinfo_actions`'s insert loop
    /// runs zero times and the character gets no `game_player_action` rows at all.
    #[test]
    fn matching_actions_on_an_empty_table_creates_nothing() {
        let rows: Vec<CreateinfoAction> = Vec::new();
        assert!(matching_actions(rows.iter(), 1, 1).is_empty());
    }

    /// Once rows ARE imported, only the ones matching (race, class) are copied — a Dwarf Warrior's
    /// row for a Human Priest's creation is never granted.
    #[test]
    fn matching_actions_copies_only_the_matching_race_and_class() {
        let rows = [
            row(1, 1, 1, 0, 6603, 0), // Human Warrior, Attack on button 0
            row(2, 1, 1, 1, 78, 0),   // Human Warrior, Heroic Strike on button 1
            row(3, 3, 1, 0, 6603, 0), // Dwarf Warrior — different race, must NOT match
            row(4, 1, 5, 0, 6603, 0), // Human Priest — different class, must NOT match
        ];
        let granted = matching_actions(rows.iter(), 1, 1);
        assert_eq!(granted, vec![(0, 6603, 0), (1, 78, 0)]);
    }

    /// Two matching rows for the SAME button (wildcard overlapping concrete, or a malformed dump)
    /// yield exactly one grant — first match wins, never two rows racing on gateway iteration order.
    #[test]
    fn matching_actions_grants_at_most_one_row_per_button() {
        let rows = [
            row(1, 1, 1, 0, 6603, 0), // Human Warrior concrete: Attack on button 0
            row(2, 0, 1, 0, 78, 0),   // race-wildcard Warrior row also claiming button 0
            row(3, 1, 1, 1, 78, 0),   // distinct button — unaffected
        ];
        assert_eq!(
            matching_actions(rows.iter(), 1, 1),
            vec![(0, 6603, 0), (1, 78, 0)]
        );
    }

    /// Wildcard rows (`race == 0` or `class == 0`) apply broadly, exactly like the spell kit —
    /// e.g. a class-wildcard row (Warrior's Attack, race 0) matches every playable race.
    #[test]
    fn matching_actions_respects_wildcards_like_the_spell_kit() {
        let rows = [row(1, 0, 1, 0, 6603, 0)]; // race-wildcard: every Warrior gets Attack
        assert_eq!(matching_actions(rows.iter(), 1, 1), vec![(0, 6603, 0)]); // Human Warrior
        assert_eq!(matching_actions(rows.iter(), 3, 1), vec![(0, 6603, 0)]); // Dwarf Warrior
        assert!(matching_actions(rows.iter(), 1, 4).is_empty()); // Human Mage — class still gates
    }
}
