//! `Coordinator`'s [`CharacterStore`] adapter.

use anyhow::Result;

use crate::codec;
use crate::world::CharacterStore;

use crate::stdb::connection::{CharacterPresenceSnapshot, Coordinator};

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
    /// `world/tests.rs`), and a fresh character has no live history to lose in transit — if
    /// anything the simplest case that machinery handles. What it did NOT have at first is a
    /// test that the first login of a freshly created character actually drives that transfer
    /// end-to-end rather than merely reusing already-tested machinery by assumption:
    /// `a_freshly_created_characters_first_login_transfers_off_the_default_shard` (`world/tests.rs`).
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
