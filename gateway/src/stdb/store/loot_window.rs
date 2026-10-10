//! `Coordinator`'s [`LootWindowStore`] adapter.

use anyhow::{anyhow, Result};
use lyracore_shared::group::GroupKind;
use lyracore_shared::loot::{LootBoundaryFailure, LootRefusal};
use spacetimedb_sdk::Table;

use crate::codec;
use crate::stdb::bindings::*;
use crate::stdb::connection::{call_reducer, reducer_refusal_reason, LiveConn};
use crate::stdb::reads::player_item_count;
use crate::stdb::Coordinator;
use crate::world::{Actor, LootWindowRefusal, LootWindowRequestStatus, LootWindowStore};

impl LootWindowStore for crate::stdb::Coordinator {
    fn loot_target_money(&self, target_guid: u64) -> Result<u32> {
        crate::stdb::Coordinator::loot_target_money(self, target_guid)
    }

    fn loot_target_items(
        &self,
        target_guid: u64,
        viewer_guid: u64,
    ) -> Result<Vec<codec::LootItemView>> {
        crate::stdb::Coordinator::corpse_loot(self, target_guid, viewer_guid)
    }

    fn use_gameobject(
        &self,
        account_id: u64,
        actor_guid: u64,
        target_guid: u64,
    ) -> Result<LootWindowRequestStatus> {
        crate::stdb::Coordinator::use_gameobject(self, account_id, actor_guid, target_guid)
    }

    fn open_creature_loot(
        &self,
        account_id: u64,
        actor_guid: u64,
        corpse_guid: u64,
    ) -> Result<LootWindowRequestStatus> {
        crate::stdb::Coordinator::open_creature_loot(self, account_id, actor_guid, corpse_guid)
    }

    fn skin_corpse(
        &self,
        account_id: u64,
        actor_guid: u64,
        target_guid: u64,
    ) -> Result<LootWindowRequestStatus> {
        crate::stdb::Coordinator::skin_corpse(self, account_id, actor_guid, target_guid)
    }

    fn loot_money(
        &self,
        account_id: u64,
        actor_guid: u64,
        target_guid: u64,
    ) -> Result<LootWindowRequestStatus> {
        crate::stdb::Coordinator::loot_money(self, account_id, actor_guid, target_guid)
    }

    fn take_loot(
        &self,
        account_id: u64,
        actor_guid: u64,
        target_guid: u64,
        loot_slot: u8,
    ) -> Result<LootWindowRequestStatus> {
        crate::stdb::Coordinator::take_loot(self, account_id, actor_guid, target_guid, loot_slot)
    }
}

impl Coordinator {
    /// Read a corpse's item loot for the loot window, joined with each item's
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
                    let needs = viewer_needs_quest_item(&guard, viewer_guid, l.item_entry);
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
                (
                    l.slot,
                    l.item_entry,
                    l.count,
                    display_id,
                    l.random_property_id,
                )
            })
            .collect();
        items.sort_by_key(|(slot, ..)| *slot); // stable loot-slot order (SQL has no ORDER BY in 2.5)
        Ok(items)
    }

    /// Read a corpse's lootable copper for `SMSG_LOOT_RESPONSE` from the privileged cache.
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

    /// Use a gameobject (`CMSG_GAMEOBJ_USE`) — a chest rolls its loot into the corpse-loot table keyed
    /// on the GO guid, a quest-use object grants quest credit. The module gates range + type.
    /// Rides the coordinator connection as `gw_use_gameobject`.
    pub fn use_gameobject(
        &self,
        _account_id: u64,
        actor_guid: u64,
        go_guid: u64,
    ) -> Result<LootWindowRequestStatus> {
        let actor = Actor::new(actor_guid)
            .ok_or_else(|| anyhow!("use_gameobject: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        legacy_loot_request_status(call_reducer!(
            coord.conn.reducers,
            "gw_use_gameobject",
            gw_use_gameobject_then(self.session_actor(actor), go_guid)
        ))
    }

    /// Take the money from a corpse (`CMSG_LOOT_MONEY`) over the coordinator connection so
    /// the module attributes the loot to the caller (as `gw_loot_money`).
    pub fn loot_money(
        &self,
        _account_id: u64,
        actor_guid: u64,
        target_guid: u64,
    ) -> Result<LootWindowRequestStatus> {
        let actor =
            Actor::new(actor_guid).ok_or_else(|| anyhow!("loot_money: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        strict_loot_request_status(call_reducer!(
            coord.conn.reducers,
            "gw_loot_money",
            gw_loot_money_then(self.session_actor(actor), target_guid)
        ))
    }

    /// Authorize opening a creature corpse before the Gateway reads its loot rows.
    pub fn open_creature_loot(
        &self,
        _account_id: u64,
        actor_guid: u64,
        corpse_guid: u64,
    ) -> Result<LootWindowRequestStatus> {
        let actor = Actor::new(actor_guid)
            .ok_or_else(|| anyhow!("open_creature_loot: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        strict_loot_request_status(call_reducer!(
            coord.conn.reducers,
            "gw_open_creature_loot",
            gw_open_creature_loot_then(self.session_actor(actor), corpse_guid)
        ))
    }

    /// Take one item from the open corpse into the backpack (`CMSG_AUTOSTORE_LOOT_ITEM`) over
    /// the coordinator connection so the module attributes the loot to the caller. The module moves the
    /// item into a free slot + deletes the corpse-loot row (the inventory relay then shows it in the bag).
    /// Rides the coordinator connection as `gw_take_loot`.
    pub fn take_loot(
        &self,
        _account_id: u64,
        actor_guid: u64,
        corpse_guid: u64,
        loot_slot: u8,
    ) -> Result<LootWindowRequestStatus> {
        let actor =
            Actor::new(actor_guid).ok_or_else(|| anyhow!("take_loot: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        strict_loot_request_status(call_reducer!(
            coord.conn.reducers,
            "gw_take_loot",
            gw_take_loot_then(self.session_actor(actor), corpse_guid, loot_slot)
        ))
    }

    pub fn skin_corpse(
        &self,
        _account_id: u64,
        actor_guid: u64,
        corpse_guid: u64,
    ) -> Result<LootWindowRequestStatus> {
        let actor =
            Actor::new(actor_guid).ok_or_else(|| anyhow!("skin_corpse: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        legacy_loot_request_status(call_reducer!(
            coord.conn.reducers,
            "gw_skin",
            gw_skin_then(self.session_actor(actor), corpse_guid)
        ))
    }
}

/// Whether `viewer_guid` is in a Raid, read from this handle's cache: the `PartyMembershipIndex`
/// (an indexed lookup, never a table scan) names the group, and the cached `game_group.group_type`
/// names its kind. The Gateway twin of the Module's `group::in_raid`.
fn viewer_in_raid(guard: &LiveConn, viewer_guid: u64) -> bool {
    let Some(group_id) = guard
        .party_memberships
        .read()
        .unwrap()
        .group_of(viewer_guid)
    else {
        return false;
    };
    guard
        .conn
        .db
        .game_group()
        .group_id()
        .find(&group_id)
        .is_some_and(|group| {
            GroupKind::from_wire(group.group_type).unwrap_or_default() == GroupKind::Raid
        })
}

/// Does `viewer_guid` currently need quest item `item_entry`? The gateway twin
/// of the module's `loot::killer_needs_item`/`needs_item_pure` (an ACTIVE — un-rewarded — quest with a
/// `COLLECT_ITEM` objective naming `item_entry`, held < required), applied to the VIEWER opening the
/// loot window rather than the credited killer, and gated by the same raid quest-credit rule
/// (`lyracore_shared::quest::quest_progresses_for_raid`) so the loot window never promises an item
/// a Raid member's `take_loot` would then refuse (that Gate is `loot::killer_needs_item`, gated
/// identically). RLS-bypassed read (coordinator), same shape as
/// `quest_objectives_complete`/`player_item_count` above.
fn viewer_needs_quest_item(guard: &LiveConn, viewer_guid: u64, item_entry: u32) -> bool {
    const COLLECT_ITEM: u8 = 1; // == module objective_kind::COLLECT_ITEM
    let db = &guard.conn.db;
    let active: Vec<u32> = db
        .game_character_quest()
        .iter()
        .filter(|q| q.character_guid == viewer_guid && !q.rewarded)
        .map(|q| q.quest_entry)
        .collect();
    if active.is_empty() {
        return false;
    }
    let in_raid = viewer_in_raid(guard, viewer_guid);
    let objectives: Vec<(u32, u32, u32)> = db
        .game_quest_objective()
        .iter()
        .filter(|o| o.kind == COLLECT_ITEM && active.contains(&o.quest_entry))
        .map(|o| {
            let quest_type = db
                .game_quest_template()
                .entry()
                .find(&o.quest_entry)
                .map_or(0, |t| t.quest_type);
            (quest_type, o.target_entry, o.required_count)
        })
        .collect();
    needs_quest_item_pure(
        in_raid,
        &objectives,
        item_entry,
        player_item_count(db, viewer_guid, item_entry),
    )
}

/// Pure decision behind `viewer_needs_quest_item`: does any of `objectives` (flattened
/// `(quest_type, target_entry, required_count)` for the viewer's active COLLECT_ITEM objectives)
/// want `item`, given the viewer's Raid membership? Split out so the raid quest-credit rule's
/// wiring is unit-testable without a live cache, mirroring the module's `needs_item_pure`.
fn needs_quest_item_pure(
    in_raid: bool,
    objectives: &[(u32, u32, u32)],
    item: u32,
    held: u32,
) -> bool {
    objectives
        .iter()
        .any(|&(quest_type, target_entry, required_count)| {
            target_entry == item
                && held < required_count.max(1)
                && lyracore_shared::quest::quest_progresses_for_raid(in_raid, quest_type)
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

#[derive(Clone, Copy)]
enum UntaggedLootRejection {
    Fatal,
    LegacyUnanswered,
}

/// Classify a Durable Request from a core whose gameplay refusals all have loot tags.
fn strict_loot_request_status(result: Result<()>) -> Result<LootWindowRequestStatus> {
    loot_request_status(result, UntaggedLootRejection::Fatal)
}

/// Preserve silent gameplay refusals from the legacy GameObject and skinning cores. Boundary
/// failures and every tagged result remain explicit, so this compatibility cannot hide them.
fn legacy_loot_request_status(result: Result<()>) -> Result<LootWindowRequestStatus> {
    loot_request_status(result, UntaggedLootRejection::LegacyUnanswered)
}

fn loot_request_status(
    result: Result<()>,
    untagged: UntaggedLootRejection,
) -> Result<LootWindowRequestStatus> {
    match result {
        Ok(()) => Ok(LootWindowRequestStatus::Applied),
        Err(error) => match reducer_refusal_reason(&error) {
            Some(reason) => {
                if LootBoundaryFailure::parse_tag(reason).is_some() {
                    return Err(error);
                }
                if let Some(refusal) = LootRefusal::parse_tag(reason) {
                    log::debug!("stdb: loot Durable Request refused: {error:#}");
                    return Ok(LootWindowRequestStatus::Refused(refusal.into()));
                }
                if reason.starts_with("loot:") {
                    return Err(error);
                }
                match untagged {
                    UntaggedLootRejection::LegacyUnanswered => {
                        log::debug!("stdb: legacy loot Durable Request refused: {error:#}");
                        Ok(LootWindowRequestStatus::Refused(
                            LootWindowRefusal::Unanswered,
                        ))
                    }
                    UntaggedLootRejection::Fatal => Err(error),
                }
            }
            None => Err(error),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{
        group_loot_row_visible_to_viewer, needs_quest_item_pure, quest_row_visible_to_viewer,
    };

    // The per-viewer loot-window visibility gate. Every other read in this
    // file goes through the coordinator's live cache (`RemoteTables`) and has no fake-cache harness to
    // unit-test against (the module crate's "never mock the ctx, extract + test pure fns" rule applies
    // here too). `quest_row_visible_to_viewer` and `needs_quest_item_pure` are the decisions worth
    // pulling out pure so they're directly testable without a live SpacetimeDB connection.

    /// `needs_quest_item_pure` mirrors the Module's raid quest-credit rule: a Raid member's
    /// non-Raid quest stops needing its item, a Raid Quest's need is unaffected, and a Party
    /// member (in_raid = false) is unaffected either way. Hand-written values, not computed by
    /// the code under test.
    #[test]
    fn needs_quest_item_pure_gates_a_raid_members_non_raid_quest() {
        let normal_quest_wants_the_item = [(0, 55, 1)]; // (quest_type, target_entry, required_count)
        let raid_quest_wants_the_item = [(62, 55, 1)];
        assert!(
            needs_quest_item_pure(false, &normal_quest_wants_the_item, 55, 0),
            "outside a Raid, a normal quest's need applies"
        );
        assert!(
            !needs_quest_item_pure(true, &normal_quest_wants_the_item, 55, 0),
            "inside a Raid, a normal quest's item need is gated off"
        );
        assert!(
            needs_quest_item_pure(true, &raid_quest_wants_the_item, 55, 0),
            "inside a Raid, a Raid Quest's need still applies"
        );
        assert!(
            !needs_quest_item_pure(false, &normal_quest_wants_the_item, 55, 1),
            "held at or above the requirement never needs it"
        );
        assert!(
            !needs_quest_item_pure(true, &raid_quest_wants_the_item, 999, 0),
            "a different item entry is never needed"
        );
    }

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

#[cfg(test)]
mod loot_reducer_tests {
    use super::*;
    use crate::stdb::connection::ReducerCallError;

    fn refusal_of(
        result: Result<()>,
        classify: fn(Result<()>) -> Result<LootWindowRequestStatus>,
    ) -> LootWindowRefusal {
        match classify(result) {
            Ok(LootWindowRequestStatus::Refused(refusal)) => refusal,
            Ok(LootWindowRequestStatus::Applied) => panic!("a Refusal was applied"),
            Err(error) => panic!("a Refusal ended the session: {error:#}"),
        }
    }

    fn rejected(reason: &str) -> Result<()> {
        Err(anyhow::Error::from(ReducerCallError::Rejected {
            operation: "gw_take_loot".to_string(),
            reason: reason.to_string(),
        })
        .context("loot window"))
    }

    #[test]
    fn every_module_refusal_tag_becomes_one_client_answer() {
        for refusal in LootRefusal::ALL {
            for classify in [
                strict_loot_request_status as fn(Result<()>) -> Result<LootWindowRequestStatus>,
                legacy_loot_request_status,
            ] {
                assert_eq!(
                    refusal_of(rejected(refusal.as_tag()), classify),
                    LootWindowRefusal::from(refusal),
                    "{refusal:?}"
                );
            }
        }
    }

    #[test]
    fn only_legacy_cores_keep_untagged_gameplay_refusals_unanswered() {
        for reason in ["it is locked", "not a beast", "inventory is full"] {
            assert_eq!(
                refusal_of(rejected(reason), legacy_loot_request_status),
                LootWindowRefusal::Unanswered,
                "{reason}"
            );
            assert!(
                strict_loot_request_status(rejected(reason)).is_err(),
                "{reason}"
            );
        }
    }

    #[test]
    fn boundary_failures_and_unknown_loot_tags_are_fatal() {
        let reasons = LootBoundaryFailure::ALL
            .into_iter()
            .map(LootBoundaryFailure::as_tag)
            .chain(["loot:newer_module_refusal"]);

        for reason in reasons {
            assert!(
                strict_loot_request_status(rejected(reason)).is_err(),
                "{reason}"
            );
            assert!(
                legacy_loot_request_status(rejected(reason)).is_err(),
                "{reason}"
            );
        }
    }

    #[test]
    fn a_timeout_is_not_answered_as_a_refusal() {
        let not_refusals = [
            anyhow::Error::from(ReducerCallError::fatal(
                "gw_take_loot reducer timed out after 10s".to_string(),
            )),
            anyhow::Error::from(ReducerCallError::fatal(
                "gw_loot_money reducer failed: transport disconnected".to_string(),
            )),
            anyhow!(
                "wrapped text that mentions {}",
                LootRefusal::LootTagIneligible.as_tag()
            ),
        ];
        for error in not_refusals {
            let text = format!("{error:#}");
            assert!(strict_loot_request_status(Err(error)).is_err(), "{text}");
        }
    }
}
