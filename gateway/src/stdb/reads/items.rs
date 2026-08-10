//! Item-instance / loot cache-accessor methods — pure code-motion split of the former
//! `reads.rs`.

use anyhow::Result;
use spacetimedb_sdk::Table;

use super::super::bindings::*;
use super::super::connection::Coordinator;
use super::player_item_count;

impl Coordinator {
    /// Read every item a character owns (items slice-1), joined with its template for the CREATE
    /// descriptors (max-durability). Read from the privileged cache (the coordinator bypasses RLS),
    /// filtered by `owner_guid` — the SDK exposes only the PK index, so iterate+filter like the other
    /// row queries. Returns the instance views ready for `build_item_create_object` + inventory slots.
    /// The character's learned skills as `(skill_line, current, max_rank)` — the self-CREATE
    /// SkillInfo block's live source. RLS-bypassed cache read like the sibling item read.
    pub fn player_skills(&self, character_guid: u64) -> Result<Vec<(u32, u16, u16)>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        Ok(db
            .game_player_skill()
            .iter()
            .filter(|s| s.character_guid == character_guid)
            .map(|s| (s.skill_line, s.current, s.max_rank))
            .collect())
    }

    pub fn player_items(&self, owner_guid: u64) -> Result<Vec<crate::codec::ItemInstanceView>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let items = db
            .game_item_instance()
            .iter()
            .filter(|i| i.owner_guid == owner_guid)
            // Skip an instance whose template is absent: a delete-template migration can leave orphaned rows
            // (e.g. synthetic ids deleted by the real-item remap), and sending a phantom item — blank/unknown
            // to the client, faulting on use/equip — is worse than dropping it. Defense-in-depth so a future
            // template delete can never push a broken item to a 5875 client.
            .filter_map(|i| {
                let tmpl = db.game_item_template().entry().find(&i.entry)?;
                Some(crate::codec::ItemInstanceView {
                    guid: i.guid,
                    entry: i.entry,
                    owner_guid: i.owner_guid,
                    slot: i.slot,
                    stack_count: i.stack_count,
                    durability: i.durability,
                    max_durability: tmpl.max_durability,
                    container_slots: tmpl.container_slots,
                })
            })
            .collect();
        Ok(items)
    }

    /// Bag slot of the item instance with `item_guid`. Reads from the coordinator's privileged cache
    /// (same source as `player_items`). Item GUIDs are globally unique so account_id isn't needed
    /// for the lookup — ownership is enforced by the module reducer on the call.
    pub fn item_slot_by_guid(&self, _account_id: u64, item_guid: u64) -> Option<u8> {
        // `collect()` forces the iterator to complete (and drop its borrow from `guard`) before `guard`
        // itself drops — same pattern as `player_items`.
        let guard = self.0.coord();
        let slots: Vec<u8> = guard
            .conn
            .db
            .game_item_instance()
            .iter()
            .filter(|i| i.guid == item_guid)
            .map(|i| i.slot)
            .collect();
        slots.into_iter().next()
    }

    /// Item-starts-quest: does the item in `owner_guid`'s inventory `slot` carry a
    /// non-zero `item_template.start_quest`? Returns `(item_guid, quest_id)` if so — the gateway's
    /// `CMSG_USE_ITEM` handler uses this to open `SMSG_QUESTGIVER_QUEST_DETAILS` (item guid as giver)
    /// INSTEAD of calling `use_item` (never both — the item isn't consumed). `None` for every ordinary
    /// item (the ubiquitous case), which falls through to the normal use-item path unchanged.
    pub fn item_start_quest(&self, owner_guid: u64, slot: u8) -> Option<(u64, u32)> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let hit: Option<(u64, u32)> = db
            .game_item_instance()
            .iter()
            .filter(|i| i.owner_guid == owner_guid && i.slot == slot)
            .find_map(|i| {
                db.game_item_template()
                    .entry()
                    .find(&i.entry)
                    .filter(|t| t.start_quest != 0)
                    .map(|t| (i.guid, t.start_quest))
            });
        hit
    }

    /// Read a corpse's item loot for the loot window (items slice-4), joined with each item's
    /// template for the display id, then filtered PER VIEWER for `quest_only` rows (quest items are
    /// per-looter, not gated on whoever got kill credit) AND group-loot rows (a live NEED/GREED
    /// roll is withheld from EVERYONE; a round-robin/master-designated row is visible only to its
    /// designee). Read from the privileged cache (the coordinator bypasses RLS), filtered by
    /// `corpse_guid` (iterate+filter, like the other row queries). Returns `(slot, item_id, count,
    /// display_id)` triples for `build_loot_response_raw`. An item whose template isn't loaded falls
    /// back to display 0 (the client still resolves the name via query).
    pub fn corpse_loot(
        &self,
        corpse_guid: u64,
        viewer_guid: u64,
    ) -> Result<Vec<crate::codec::LootItemView>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let mut items: Vec<crate::codec::LootItemView> = db
            .game_corpse_loot()
            .iter()
            .filter(|l| l.corpse_guid == corpse_guid)
            .filter(|l| {
                if l.quest_only {
                    let needs = viewer_needs_quest_item(db, viewer_guid, l.item_entry);
                    quest_row_visible_to_viewer(l.quest_only, l.reserved_for, viewer_guid, needs)
                } else {
                    group_loot_row_visible_to_viewer(
                        l.reserved_for,
                        l.withheld,
                        l.master_only,
                        l.designated_looter_guid,
                        viewer_guid,
                    )
                }
            })
            .map(|l| {
                let display_id = db
                    .game_item_template()
                    .entry()
                    .find(&l.item_entry)
                    .map(|t| t.display_id)
                    .unwrap_or(0);
                (l.slot, l.item_entry, l.count, display_id)
            })
            .collect();
        items.sort_by_key(|(slot, ..)| *slot); // stable loot-slot order (SQL has no ORDER BY in 2.5)
        Ok(items)
    }

    /// Read a corpse's lootable copper for `SMSG_LOOT_RESPONSE` (slice 3) from the privileged cache.
    /// Returns 0 if the target is missing or not a corpse — the client only sends `CMSG_LOOT` on a
    /// lootable corpse, but we stay defensive (an empty loot window is harmless).
    pub fn loot_target_money(&self, target_guid: u64) -> Result<u32> {
        Ok(self
            .0
            .coord()
            .conn
            .db
            .game_world_entity()
            .guid()
            .find(&target_guid)
            .filter(|e| e.dead)
            .map(|e| e.money)
            .unwrap_or(0))
    }

    /// The player's buyback ring, newest-first: `(item_entry, stack_count, price)` ≤12.
    pub fn buyback_ring(&self, player_guid: u64) -> Vec<(u32, u32, u32)> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let mut rows: Vec<_> = db
            .game_character_buyback()
            .iter()
            .filter(|b| b.player_guid == player_guid)
            .collect();
        rows.sort_by(|a, b| b.id.cmp(&a.id));
        rows.into_iter()
            .map(|b| (b.item_entry, b.stack_count, b.price))
            .collect()
    }
}

/// Does `viewer_guid` currently need quest item `item_entry`? The gateway twin
/// of the module's `loot::killer_needs_item`/`needs_item_pure` (an ACTIVE — un-rewarded — quest with a
/// `COLLECT_ITEM` objective naming `item_entry`, held < required), applied to the VIEWER opening the
/// loot window rather than the credited killer. RLS-bypassed read (coordinator), same shape as
/// `quest_objectives_complete`/`player_item_count` above.
fn viewer_needs_quest_item(db: &RemoteTables, viewer_guid: u64, item_entry: u32) -> bool {
    const COLLECT_ITEM: u8 = 1; // == module objective_kind::COLLECT_ITEM
    let active: Vec<u32> = db
        .game_character_quest()
        .iter()
        .filter(|q| q.character_guid == viewer_guid && !q.rewarded)
        .map(|q| q.quest_entry)
        .collect();
    if active.is_empty() {
        return false;
    }
    db.game_quest_objective().iter().any(|o| {
        o.kind == COLLECT_ITEM
            && o.target_entry == item_entry
            && active.contains(&o.quest_entry)
            && player_item_count(db, viewer_guid, item_entry) < o.required_count.max(1)
    })
}

/// The per-viewer loot-window visibility gate: is a `game_corpse_loot` row
/// visible to `viewer_guid`? A non-quest row is always visible (FFA, unconditional). A `quest_only`
/// row is visible when EITHER it's still the UNRESERVED shared row
/// (`reserved_for == 0` — nobody has split it yet) and the viewer currently needs the item
/// (`viewer_needs_item`, resolved by the caller — mirrors the module's ctx/pure split so this decision
/// needs no live cache to unit-test), OR it's the viewer's OWN already-split reserved clone
/// (`reserved_for == viewer_guid`) — a clone reserved for someone ELSE is invisible regardless of need.
/// Mirrored module-side by `loot::quest_take_allowed` (same predicate shape) so a row a viewer's
/// window shows is always a row their take can actually succeed on. Pure.
pub(crate) fn quest_row_visible_to_viewer(
    quest_only: bool,
    reserved_for: u64,
    viewer_guid: u64,
    viewer_needs_item: bool,
) -> bool {
    if !quest_only {
        return true;
    }
    if reserved_for == viewer_guid {
        return true;
    }
    reserved_for == 0 && viewer_needs_item
}

/// The per-viewer loot-window visibility gate for a NON-quest row (the
/// caller only calls this when `!quest_only`; `quest_row_visible_to_viewer` handles quest rows,
/// unchanged, above). `reserved_for` is GENERALIZED here beyond the quest clone: nonzero also covers
/// a NEED/GREED winner's inventory-full fallback row (`resolve_roll`'s module doc) — either way,
/// nonzero means "visible ONLY to that guid", full stop. A `withheld` row (a live roll in progress)
/// is invisible to EVERYONE (not just non-eligible viewers — matches the design's "withheld rows
/// invisible in EVERYONE's window until resolved", proving the AutoLoot-safety trap: a grey +
/// green drop on one corpse has the grey's row visible/FFA while the green's is withheld). A
/// `master_only` row is never shown via the plain window (the master acts through
/// `SMSG_LOOT_MASTER_LIST`/`CMSG_LOOT_MASTER_GIVE` instead). A `designated_looter_guid` row (round-
/// robin/below-threshold) is visible only to its designee. The all-zero/false baseline is plain FFA
/// (byte-identical to the original baseline). Mirrored module-side by `loot::group_loot_take_allowed`
/// (same predicate shape) so a row a viewer's window shows is always a row their take can succeed
/// on. Pure.
pub(crate) fn group_loot_row_visible_to_viewer(
    reserved_for: u64,
    withheld: bool,
    master_only: bool,
    designated_looter_guid: u64,
    viewer_guid: u64,
) -> bool {
    if reserved_for != 0 {
        return reserved_for == viewer_guid;
    }
    if withheld || master_only {
        return false;
    }
    designated_looter_guid == 0 || designated_looter_guid == viewer_guid
}

#[cfg(test)]
mod tests {
    use super::{group_loot_row_visible_to_viewer, quest_row_visible_to_viewer};

    // The per-viewer loot-window visibility gate. Every other read in this
    // file goes through the coordinator's live cache (`RemoteTables`) and has no fake-cache harness to
    // unit-test against (the module crate's "never mock the ctx, extract + test pure fns" rule applies
    // here too) — `quest_row_visible_to_viewer` is the one decision worth pulling out pure so it's
    // directly testable without a live SpacetimeDB connection.

    #[test]
    fn non_quest_rows_are_always_visible_regardless_of_viewer_state() {
        // FFA, unconditional — the original baseline behavior. reserved_for/need are irrelevant.
        assert!(quest_row_visible_to_viewer(false, 0, 7, false));
        assert!(quest_row_visible_to_viewer(false, 99, 7, false));
        assert!(quest_row_visible_to_viewer(false, 7, 7, false));
    }

    #[test]
    fn an_unreserved_quest_row_is_visible_only_to_a_needing_viewer() {
        assert!(
            quest_row_visible_to_viewer(true, 0, 7, true),
            "needs it -> sees the shared row"
        );
        assert!(
            !quest_row_visible_to_viewer(true, 0, 7, false),
            "doesn't need it -> invisible"
        );
    }

    #[test]
    fn a_reserved_clone_is_visible_only_to_its_owner() {
        // The viewer's OWN clone is visible even if `viewer_needs_item` somehow reads false (a stale
        // held-count race) — the reservation itself is the authority once split.
        assert!(
            quest_row_visible_to_viewer(true, 7, 7, false),
            "the viewer's own reserved clone"
        );
        assert!(quest_row_visible_to_viewer(true, 7, 7, true));
        // Reserved for a DIFFERENT character — invisible to this viewer even if they also need it.
        assert!(
            !quest_row_visible_to_viewer(true, 7, 8, true),
            "reserved for someone else"
        );
        assert!(!quest_row_visible_to_viewer(true, 7, 8, false));
    }

    // ---- Group loot methods ----

    #[test]
    fn group_loot_baseline_ffa_is_visible_to_everyone() {
        assert!(group_loot_row_visible_to_viewer(0, false, false, 0, 7));
        assert!(group_loot_row_visible_to_viewer(0, false, false, 0, 8));
    }

    #[test]
    fn group_loot_withheld_row_is_invisible_to_everyone_including_the_eventual_winner() {
        // The AutoLoot-safety trap: a live roll's row must not appear in ANYONE's window, not even
        // the guid who will eventually win it.
        assert!(!group_loot_row_visible_to_viewer(0, true, false, 0, 7));
        assert!(!group_loot_row_visible_to_viewer(0, true, false, 7, 7));
    }

    #[test]
    fn group_loot_master_only_row_never_shows_in_the_plain_window() {
        // Not even the stamped master sees it here — they act via SMSG_LOOT_MASTER_LIST instead.
        assert!(!group_loot_row_visible_to_viewer(0, false, true, 42, 42));
        assert!(!group_loot_row_visible_to_viewer(0, false, true, 42, 8));
    }

    #[test]
    fn group_loot_designated_row_is_visible_only_to_its_designee() {
        assert!(group_loot_row_visible_to_viewer(0, false, false, 42, 42));
        assert!(!group_loot_row_visible_to_viewer(0, false, false, 42, 8));
    }

    #[test]
    fn group_loot_reserved_winner_locked_row_wins_over_every_other_flag() {
        // A nonzero reserved_for (the inventory-full winner fallback) is exclusive and unconditional
        // — it overrides withheld/master_only/designated entirely (those are all stale by then).
        assert!(group_loot_row_visible_to_viewer(7, true, true, 99, 7));
        assert!(!group_loot_row_visible_to_viewer(7, true, true, 99, 8));
    }

    /// The AutoLoot-addon trap, spelled out as ONE corpse's two rows: a grey
    /// (below-threshold, FFA-in-group) item autoloots normally while a green (at/above threshold,
    /// mid-roll) item on the SAME corpse is simultaneously withheld from every window — proving the
    /// two rows are decided independently and a live roll can never be swept up by autostore.
    #[test]
    fn a_grey_autoloots_while_a_green_on_the_same_corpse_is_withheld() {
        let grey = (0u64, false, false, 0u64); // FFA baseline: (reserved_for, withheld, master_only, designated)
        let green = (0u64, true, false, 0u64); // a live NEED/GREED roll in progress
        for viewer in [7u64, 8u64] {
            assert!(
                group_loot_row_visible_to_viewer(grey.0, grey.1, grey.2, grey.3, viewer),
                "the grey row autoloots for any viewer"
            );
            assert!(
                !group_loot_row_visible_to_viewer(green.0, green.1, green.2, green.3, viewer),
                "the green row is withheld from every viewer while its roll is live"
            );
        }
    }

}
