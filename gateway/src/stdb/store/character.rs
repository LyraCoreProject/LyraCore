//! `Coordinator`'s [`CharacterStore`] adapter.

use anyhow::Result;
use spacetimedb_sdk::Table;

use crate::codec;
use crate::stdb::bindings::*;
use crate::stdb::connection::{call_reducer, CharacterPresenceSnapshot, Coordinator};
use crate::stdb::reads::spell_ranks_stack_in_book;
use crate::stdb::views::character_view;
use crate::world::CharacterStore;

fn stable_character_absence(
    first: &[CharacterPresenceSnapshot],
    second: &[CharacterPresenceSnapshot],
) -> bool {
    first.len() == second.len()
        && first.iter().zip(second).all(|(before, after)| {
            !before.present
                && !after.present
                && before.generation == after.generation
                && before.revision == after.revision
        })
}

fn durable_character_presence_snapshots(
    shards: &[(String, Coordinator)],
    guid: u64,
) -> Result<Vec<CharacterPresenceSnapshot>> {
    shards
        .iter()
        .map(|(name, shard)| {
            shard
                .0
                .coord()
                .durable_character_presence_snapshot(guid, name)
        })
        .collect()
}

impl CharacterStore for Coordinator {
    /// Union the account's Characters across connected World Shards. Deduplicate by guid
    /// during interrupted Transfers, keeping the first copy in default-first probe order.
    /// Guild membership comes from Realm-core; an unavailable authority leaves guild names empty.
    fn characters(&self, account_id: u64) -> Result<Vec<codec::CharacterView>> {
        let mut out: Vec<codec::CharacterView> = if self.is_sharded() {
            let mut out: Vec<codec::CharacterView> = Vec::new();
            for shard in self.all_shards() {
                for c in shard.characters(account_id)? {
                    if !out.iter().any(|existing| existing.guid == c.guid) {
                        out.push(c);
                    }
                }
            }
            out
        } else {
            self.characters(account_id)?
        };
        match self.realm_core() {
            Ok(realm) => {
                for c in &mut out {
                    c.guild_id = realm.guild_projection(c.guid).0;
                }
            }
            Err(error) => log::warn!("world: character list without guild ids: {error:#}"),
        }
        Ok(out)
    }

    /// Create a character on `self` — the DEFAULT/realm shard — always, even when the race's start
    /// position (`module/src/auth.rs`'s per-race spawn table) routes to a DIFFERENT world shard
    /// under the configured `LYRACORE_SHARD_MAP` (e.g. an Orc/Tauren/Troll/Night Elf's Kalimdor
    /// start position on a two-continent split). The DECISION, made explicit:
    ///
    /// **Create-then-transfer-on-first-login, not create-directly-on-the-owning-shard.** The row
    /// lands on `self` and stays there until the character's very first `CMSG_PLAYER_LOGIN` runs
    /// `route_home`/`settle_home_shard` — the SAME resolve-and-transfer every OTHER
    /// login already goes through, freshly created or not. Creating directly on the owning shard
    /// instead would need its own routing resolved BEFORE the character exists (nothing to look up
    /// in the character→shard index yet), built from scratch for a path that runs exactly once per
    /// character and is not latency-sensitive the way login is.
    ///
    /// This costs nothing NEW: the escrowed transfer it rides is the one already proven against a
    /// full gateway-kill crash matrix
    /// (`a_gateway_kill_at_every_transfer_step_recovers_to_exactly_one_whole_copy`,
    /// `world/transfer_tests.rs`), and a fresh character has no live history to lose in transit — if
    /// anything the simplest case that machinery handles. What it did NOT have at first is a
    /// test that the first login of a freshly created character actually drives that transfer
    /// end-to-end rather than merely reusing already-tested machinery by assumption:
    /// `a_freshly_created_characters_first_login_transfers_off_the_default_shard`
    /// (`world/shard_routing_tests.rs`).
    fn create_character(
        &self,
        account_id: u64,
        name: &str,
        race: u8,
        class: u8,
        gender: u8,
        appearance: codec::Appearance,
    ) -> Result<codec::CharCreateOutcome> {
        self.create_character(account_id, name, race, class, gender, appearance)
    }

    /// Delete a character wherever it actually LIVES, not only on `self`.
    ///
    /// `characters()` above is a cross-shard UNION: a character resident on ANY connected
    /// shard shows at char-select. `delete_character` did not match that — the reducer
    /// call only ever ran on `self`, so a character resident on a non-default shard was NOT_FOUND
    /// there (surfaced as `Failed`) even though the row was real and deletable.
    /// The resolution is `realm_core::resolve_delete_shard` — the SAME index-first
    /// lookup (`locate_home_shard`) the world-entry path already trusts, not a second routing
    /// mechanism — and delete runs EXACTLY ONCE, on the resolved owner: turning "the wrong shard"
    /// into "every shard" would trade a correctness bug for a data-loss footgun. A character caught
    /// mid-transfer (an in-flight escrow) is refused rather than raced — see
    /// `resolve_delete_shard`'s own doc for why.
    ///
    /// `account_id` is forwarded UNCHANGED to whichever shard `resolve_delete_shard` names — not
    /// re-resolved for that database. This is safe, not merely assumed: `game_character.account_id`
    /// is a per-database `#[auto_inc]` surrogate in general (`realm_core::lookup_session`'s doc
    /// covers that boundary — the REALM-CORE ↔ world-shard one, where ids legitimately differ), but
    /// a character can only ever REACH a non-default world shard through the escrowed transfer
    /// (`create_character` never targets one directly — see its own doc comment), and
    /// `ensure_shadow_account` (`module/src/auth.rs`) creates that shard's shadow `game_account` row
    /// with the EXACT numeric id carried in the import blob, bypassing `#[auto_inc]` — so
    /// `account_id` is preserved bit-for-bit, world-shard to world-shard, across every transfer.
    /// `characters()`'s cross-shard union above and `route_home`'s `bind_shard_session` already
    /// rely on this same invariant (`world/mod.rs`), so this is the established precedent, not a
    /// new assumption introduced here.
    fn delete_character(
        &self,
        account_id: u64,
        character_guid: u64,
    ) -> Result<codec::CharDeleteOutcome> {
        match crate::realm_core::resolve_delete_shard(self, character_guid) {
            Ok(Some(owner)) => owner.delete_character(account_id, character_guid),
            Ok(None) => self.delete_character(account_id, character_guid),
            Err(e) => {
                log::warn!("world: {e:#}");
                Ok(codec::CharDeleteOutcome::Failed)
            }
        }
    }

    fn character_by_guid(&self, guid: u64) -> Result<Option<codec::CharacterView>> {
        self.character_by_guid(guid)
    }

    fn character_exists_on_any_world_shard(&self, guid: u64) -> Result<bool> {
        let shards = self.world_shards_for_absence()?;
        let first = durable_character_presence_snapshots(&shards, guid)?;
        if first.iter().any(|snapshot| snapshot.present) {
            return Ok(true);
        }
        let second = durable_character_presence_snapshots(&shards, guid)?;
        if second.iter().any(|snapshot| snapshot.present) {
            return Ok(true);
        }
        if !stable_character_absence(&first, &second) {
            anyhow::bail!(
                "World Shard Character presence changed while checking {guid}; cleanup is deferred"
            );
        }
        Ok(false)
    }

    fn player_skills(&self, character_guid: u64) -> Result<Vec<(u32, u16, u16)>> {
        self.player_skills(character_guid)
    }

    fn effective_armor(&self, guid: u64) -> u32 {
        self.effective_armor(guid)
    }

    fn effective_magic_resistances(&self, guid: u64) -> [u32; 6] {
        self.effective_magic_resistances(guid)
    }

    fn spell_modifiers(&self, character_guid: u64) -> Vec<(u32, u8, i32, bool)> {
        self.spell_modifiers(character_guid)
    }

    fn player_learned_spells(&self, player_guid: u64) -> Result<Vec<u32>> {
        self.player_learned_spells(player_guid)
    }

    fn player_reputations(&self, player_guid: u64) -> Result<Vec<(i32, i32, bool)>> {
        self.player_reputations(player_guid)
    }

    fn player_actions(&self, player_guid: u64) -> Result<Vec<(u8, u32, u8)>> {
        self.player_actions(player_guid)
    }
}

impl Coordinator {
    /// Read an account's characters for the character-select screen. In production
    /// this reads the per-player `game_character` subscription cache (RLS-restricted to owner).
    /// Equipment slots (0..=18) are populated from `game_item_instance` + `game_item_template`
    /// so the client renders the character's gear on the select screen instead of all-naked.
    pub fn characters(&self, account_id: u64) -> Result<Vec<crate::codec::CharacterView>> {
        use wow_world_messages::vanilla::{CharacterGear, InventoryType};
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let mut views: Vec<crate::codec::CharacterView> = db
            .game_character()
            .iter()
            .filter(|c| c.account_id == account_id)
            .map(character_view)
            .collect();
        // Fill equipment slots 0..=18 from item instances.  Slots ≥ 19 are backpack/bag slots;
        // skip them.  An unknown inventory_type degrades to InventoryType::default (Non) which
        // the client treats the same as display_id=0 — no model shown, no crash.
        for view in &mut views {
            for item in db
                .game_item_instance()
                .iter()
                .filter(|i| i.owner_guid == view.guid && i.slot <= 18)
            {
                let Some(tmpl) = db.game_item_template().entry().find(&item.entry) else {
                    continue;
                };
                let inv_type =
                    InventoryType::try_from(u32::from(tmpl.inventory_type)).unwrap_or_default();
                view.equipment[item.slot as usize] = CharacterGear {
                    equipment_display_id: tmpl.display_id,
                    inventory_type: inv_type,
                };
            }
        }
        Ok(views)
    }

    /// Read a single character by guid (any owner) for a `CMSG_NAME_QUERY` reply. The queried guid
    /// is usually a *peer*, so this reads across owners via the privileged cache (no RLS on the
    /// owner connection), unlike `characters` which filters to one account.
    pub fn character_by_guid(&self, guid: u64) -> Result<Option<crate::codec::CharacterView>> {
        Ok(self
            .0
            .coord()
            .conn
            .db
            .game_character()
            .guid()
            .find(&guid)
            .map(character_view))
    }

    /// folded from the coordinator's privileged cache: base (`game_world_entity.armor`) + worn gear armor.
    /// The coordinator isn't subscribed to `game_aura`, so the aura term is 0 here — login-present armor
    /// auras are pushed by the on_aura relay the instant they insert. Delegates to the shared
    /// `stdb::armor::effective_armor` so CREATE and the relays compute the IDENTICAL fold.
    pub fn effective_armor(&self, guid: u64) -> u32 {
        let guard = self.0.coord();
        super::super::armor::effective_armor(&guard.conn.db, guid)
    }

    pub fn effective_magic_resistances(&self, guid: u64) -> [u32; 6] {
        let guard = self.0.coord();
        super::super::armor::effective_magic_resistances(&guard.conn.db, guid)
    }

    /// The character's active SPELL-MODIFIER auras as raw `(family_mask, op, amount, is_pct)`
    /// rows — the client-mirror source for SMSG_SET_FLAT/PCT_SPELL_MODIFIER (aggregation by
    /// (op, mask-bit) happens in the codec helper; mangos sends the TOTAL per bit).
    pub fn spell_modifiers(&self, character_guid: u64) -> Vec<(u32, u8, i32, bool)> {
        const A_SPELLMOD_FLAT: u8 = 0xAC; // lockstep with module taxonomy
        const A_SPELLMOD_PCT: u8 = 0xAD;
        self.0
            .coord()
            .conn
            .db
            .game_aura()
            .iter()
            .filter(|a| {
                a.target_guid == character_guid
                    && (a.eff_kind == A_SPELLMOD_FLAT || a.eff_kind == A_SPELLMOD_PCT)
            })
            .map(|a| {
                (
                    a.eff_p1 as u32,
                    a.eff_p0 as u8,
                    a.amount,
                    a.eff_kind == A_SPELLMOD_PCT,
                )
            })
            .collect()
    }

    /// The player's LEARNED spells — `game_player_spell` rows for this character (the coordinator
    /// bypasses RLS so it reads any player's). Chained into the login spellbook so a taught ability
    /// (Auto Shot) reaches the client and `CastSpellByName` can fire it.
    pub fn player_learned_spells(&self, player_guid: u64) -> Result<Vec<u32>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let known: Vec<u32> = db
            .game_player_spell()
            .iter()
            .filter(|s| s.character_guid == player_guid)
            .map(|s| s.spell_id)
            .collect();
        // 258 rank collapse: drop a known rank that another KNOWN spell supersedes (a game_spell_chain
        // row whose prev_spell is this id) — GATED on the same cmangos stacking rule as
        // superseded_old_rank (operator-corrected): MANA spells keep every rank in the book
        // (downranking Holy Light is a real thing); only non-mana/passive chains collapse
        // (Heroic Strike). One pass suffices: each superseded rank is prev of its own successor.
        let known_set: std::collections::HashSet<u32> = known.iter().copied().collect();
        let superseded: std::collections::HashSet<u32> = db
            .game_spell_chain()
            .iter()
            .filter(|c| {
                c.prev_spell != 0
                    && known_set.contains(&c.prev_spell)
                    && known_set.contains(&c.spell_id)
                    && !spell_ranks_stack_in_book(db, c.spell_id)
            })
            .map(|c| c.prev_spell)
            .collect();
        Ok(known
            .into_iter()
            .filter(|id| !superseded.contains(id))
            .collect())
    }

    /// The player's IMPORTED action-bar rows as `(button, action, action_type)` triples —
    /// `game_player_action` rows copied at character creation from `game_createinfo_action` (empty when
    /// no dump has been imported, the common case today). Chained into the login codec
    /// (`login_sequence_messages`), which builds the bar from these when non-empty and falls back to
    /// the spellbook synth otherwise. RLS-bypassed read, like `player_learned_spells`.
    pub fn player_actions(&self, player_guid: u64) -> Result<Vec<(u8, u32, u8)>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        Ok(db
            .game_player_action()
            .iter()
            .filter(|a| a.character_guid == player_guid)
            .map(|a| (a.button, a.action, a.action_type))
            .collect())
    }

    /// The player's persisted reputation standings as `(reputation_index, standing,
    /// at_war)` triples — chained into the login `SMSG_INITIALIZE_FACTIONS` so a relog carries the
    /// real standing + the At-War checkbox instead of the all-neutral stub. Rows with
    /// `reputation_index < 0` (stale pre-migration filler) are skipped — there is no slot to
    /// address. RLS-bypassed read, like `player_learned_spells`.
    pub fn player_reputations(&self, player_guid: u64) -> Result<Vec<(i32, i32, bool)>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        Ok(db
            .game_player_reputation()
            .iter()
            .filter(|r| r.character_guid == player_guid && r.reputation_index >= 0)
            .map(|r| (r.reputation_index, r.standing, r.at_war))
            .collect())
    }

    /// Does `character_guid` sell, or lead the bidding on, an Auction on THIS handle?
    fn has_auction(&self, character_guid: u64) -> bool {
        let auctions = self.auction_keys(|index| &index.auctions, character_guid);
        let guard = self.0.coord();
        auctions.into_iter().any(|id| {
            u32::try_from(id)
                .ok()
                .and_then(|id| guard.conn.db.game_auction().id().find(&id))
                .is_some_and(|auction| {
                    auction.owner_guid == character_guid
                        || auction.highest_bidder_guid == character_guid
                })
        })
    }

    /// Create a character via the `create_character` reducer (owner connection), mapping the
    /// reducer result to a game outcome. A distinguished `NAME_IN_USE` error → `NameInUse`; any
    /// other reducer/transport error → `Failed` (never propagated as a hard error, so a bad
    /// creation can't drop the world session).
    pub fn create_character(
        &self,
        account_id: u64,
        name: &str,
        race: u8,
        class: u8,
        gender: u8,
        appearance: crate::codec::Appearance,
    ) -> Result<crate::codec::CharCreateOutcome> {
        use crate::codec::CharCreateOutcome;
        // The SpacetimeDB-generated reducer binding takes the five appearance bytes positionally;
        // unbundle `Appearance` here, at the single generated-boundary call.
        let result = call_reducer!(
            self.0.call_pipe().conn.reducers,
            "create_character",
            create_character_then(
                account_id,
                name.to_string(),
                race,
                class,
                gender,
                appearance.skin,
                appearance.face,
                appearance.hair_style,
                appearance.hair_color,
                appearance.facial_hair
            )
        );
        Ok(match result {
            Ok(()) => CharCreateOutcome::Success,
            Err(e) if e.to_string().contains("NAME_IN_USE") => CharCreateOutcome::NameInUse,
            Err(e) if e.to_string().contains("SERVER_LIMIT") => CharCreateOutcome::ServerLimit,
            // The 5875 client has no code for "this database may not mint guids", so the outcome is
            // the generic failure — but the REASON must not be swallowed: the whole point of
            // guid-range licensing is that an unlicensed shard fails loudly instead of minting
            // into someone else's range.
            Err(e) => {
                log::warn!("create_character on {} failed: {e:#}", self.shard_name());
                CharCreateOutcome::Failed
            }
        })
    }

    /// Delete a character via the `delete_character` reducer (owner connection — the reducer is
    /// operator-gated, mirroring `create_character`). Ownership is enforced module-side (`NOT_OWNER`
    /// if `character_guid` isn't `account_id`'s), so a malicious/buggy client can't delete another
    /// account's character. Maps to a game outcome the same way `create_character` does: never
    /// propagated as a hard error, so a bad delete can't drop the world session.
    pub fn delete_character(
        &self,
        account_id: u64,
        character_guid: u64,
    ) -> Result<crate::codec::CharDeleteOutcome> {
        use crate::codec::CharDeleteOutcome;
        match self.character_has_auction_value(character_guid) {
            Ok(true) => return Ok(CharDeleteOutcome::Failed),
            Ok(false) => {}
            Err(error) => {
                log::warn!(
                    "delete_character: could not verify auction value for {character_guid}: {error:#}"
                );
                return Ok(CharDeleteOutcome::Failed);
            }
        }
        let result = call_reducer!(
            self.0.call_pipe().conn.reducers,
            "delete_character",
            delete_character_then(account_id, self.actor_or_owner(character_guid))
        );
        Ok(match result {
            Ok(()) => CharDeleteOutcome::Success,
            // The 1.12 client has one reason for every refusal, so the reason goes to the log:
            // CHAR_HAS_GUILD_FEE_HOLD clears when the Character next enters the world and the
            // Gateway finishes its Fee Hold.
            Err(error) => {
                log::info!("delete_character: {character_guid} not deleted: {error:#}");
                CharDeleteOutcome::Failed
            }
        })
    }

    fn character_has_auction_value(&self, character_guid: u64) -> Result<bool> {
        for (_, shard) in self.world_shards() {
            if !shard.listing_holds(character_guid).is_empty()
                || shard
                    .unfinished_auction_holds(character_guid)
                    .next()
                    .is_some()
            {
                return Ok(true);
            }
        }
        Ok(self.realm_core()?.has_auction(character_guid))
    }
}

#[cfg(test)]
mod stable_absence_tests {
    #[test]
    fn a_transfer_between_durable_scans_cannot_prove_stable_absence() {
        use super::{stable_character_absence, CharacterPresenceSnapshot};

        let snapshot = |generation, revision, present| CharacterPresenceSnapshot {
            generation,
            revision,
            present,
        };
        let first = [snapshot(1, 0, false), snapshot(2, 1, false)];
        let destination_visible = [snapshot(1, 1, true), snapshot(2, 1, false)];
        assert!(!stable_character_absence(&first, &destination_visible));

        let moved_again = [snapshot(1, 2, false), snapshot(2, 2, false)];
        assert!(!stable_character_absence(&first, &moved_again));

        let unchanged = [snapshot(1, 0, false), snapshot(2, 1, false)];
        assert!(stable_character_absence(&first, &unchanged));
    }
}
