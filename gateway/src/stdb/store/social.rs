//! `Coordinator`'s [`SocialStore`] adapter.

use anyhow::Result;
use lyracore_shared::social::ContactRefusal;
use spacetimedb_sdk::Table;

use crate::stdb::bindings::*;
use crate::stdb::connection::call_reducer;
use crate::stdb::Coordinator;
use crate::stdb::{classify, DurableFailure};
use crate::world::{Actor, ContactOutcome, SocialStore};

impl SocialStore for Coordinator {
    /// This Shard's durable Character row for `guid`: identity plus the session flag. `None` if
    /// this Shard holds no `game_character` row for it.
    fn character_identity(
        &self,
        guid: u64,
    ) -> Result<Option<crate::world::presence::CharacterIdentity>> {
        Ok(self
            .0
            .coord()
            .conn
            .db
            .game_character()
            .guid()
            .find(&guid)
            .map(|ch| crate::world::presence::CharacterIdentity {
                guid,
                name: ch.name,
                race: ch.race,
                class: ch.class,
                level: ch.level,
                zone_id: ch.zone_id,
                session_online: ch.online,
            }))
    }

    /// This Shard's live `game_world_entity` row for `guid`, if any — the Member Stats columns
    /// [`crate::codec::MemberEntity`] carries, joined with nothing else: level and zone come
    /// straight off the entity, current unlike the durable row.
    fn live_entity(&self, guid: u64) -> Option<crate::codec::MemberEntity> {
        let guard = self.0.coord();
        let entity = guard.conn.db.game_world_entity().guid().find(&guid)?;
        Some(crate::codec::MemberEntity {
            health: entity.health,
            max_health: entity.max_health,
            power: entity.power,
            max_power: entity.max_power,
            unit_bytes_0: entity.unit_bytes_0,
            level: entity.level,
            zone_id: entity.zone_id,
            x: entity.x,
            y: entity.y,
            dead: entity.dead,
            player_flags: entity.player_flags,
            // Member Stats' own aura/pet overlay (`Coordinator::with_member_shard_stats`) fills
            // these afterward, keyed by the `ShardId` this generic, guid-only read cannot carry.
            ..Default::default()
        })
    }

    /// Does this Shard show `guid` between two places: its own Character row reading online with
    /// no live entity here, or a Transfer Intent naming a session-less bot mid-crossing? The
    /// Transfer Intent table is bounded by the Module's writer Gate, so the scan is short.
    fn character_in_transit(&self, guid: u64) -> bool {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let session_online = db
            .game_character()
            .guid()
            .find(&guid)
            .is_some_and(|character| character.online);
        session_online
            || db
                .game_bot_transfer_intent()
                .iter()
                .any(|intent| intent.bot_guid == guid)
    }

    /// The stored Auto-Reply for `guid` on this Shard: one primary-key read of the private
    /// `game_character_away` cache.
    fn auto_reply_text(&self, guid: u64) -> Result<Option<String>> {
        Ok(self
            .0
            .coord()
            .conn
            .db
            .game_character_away()
            .character_guid()
            .find(&guid)
            .map(|row| row.message))
    }

    fn every_shard_vouches_for_absence(&self) -> Result<()> {
        self.world_shards_for_absence()?;
        Ok(())
    }

    /// Every in-world player Character on this Shard, for `CMSG_WHO → SMSG_WHO`
    /// (`presence::in_world_characters`'s per-Shard input, replacing the former `online_players`).
    /// Iterates `game_world_entity` for entries with `entry == 0` (player entities; creatures have a
    /// non-zero entry), then joins each against `game_character`. The coordinator bypasses RLS so it
    /// sees every player's entity regardless of the caller's scope.
    fn in_world_players(&self) -> Result<Vec<crate::world::presence::RealmPresence>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let shard_name = self.shard_name().to_string();
        let rows = db
            .game_world_entity()
            .iter()
            .filter(|e| e.entry == 0) // players have entry == 0; creatures have a template entry
            .filter_map(|e| {
                let ch = db.game_character().guid().find(&e.guid)?;
                let away = crate::world::presence::away_from_player_flags(e.player_flags);
                let entity = crate::codec::MemberEntity {
                    health: e.health,
                    max_health: e.max_health,
                    power: e.power,
                    max_power: e.max_power,
                    unit_bytes_0: e.unit_bytes_0,
                    level: e.level,
                    zone_id: e.zone_id,
                    x: e.x,
                    y: e.y,
                    dead: e.dead,
                    player_flags: e.player_flags,
                    // `/who` does not read auras or a pet, so this bulk scan never fills them.
                    ..Default::default()
                };
                Some(crate::world::presence::RealmPresence {
                    guid: e.guid,
                    name: ch.name,
                    race: ch.race,
                    class: ch.class,
                    level: u8::try_from(e.level).unwrap_or(u8::MAX),
                    zone_id: e.zone_id,
                    session_online: ch.online,
                    whereabouts: crate::world::presence::Whereabouts::InWorld {
                        away,
                        entity,
                        shard_name: shard_name.clone(),
                    },
                })
            })
            .collect();
        Ok(rows)
    }

    /// `game_area.name` for `zone_id`, `/who`'s search-string match against a zone name — a static
    /// catalogue, subscribed unconditionally. Empty when the catalogue holds no row for it.
    fn zone_name(&self, zone_id: u32) -> String {
        self.0
            .coord()
            .conn
            .db
            .game_area()
            .id()
            .find(&zone_id)
            .map(|a| a.name)
            .unwrap_or_default()
    }

    /// `owner_guid`'s friend guids and ignore guids for `CMSG_FRIEND_LIST`, per
    /// [`crate::world::SocialStore::contact_lists`]. `owner_guid` is always the CALLING
    /// World Session's own guid, which is always live and registered right now, so this reads the
    /// guids off its Gateway-side `Viewer` — the friend and ignore sets `world_view`'s contact
    /// Relay already keeps current — instead of scanning `game_character_contact`. `None` Viewer
    /// (should not happen for this caller) degrades to two empty lists rather than erroring.
    /// Presence composition (online/team/Away Status) lives over the Store seam in
    /// `world::social::friend_views`, not here, so a Store Fake exercises the same code a
    /// Coordinator does.
    fn contact_lists(&self, owner_guid: u64) -> Result<(Vec<u64>, Vec<u64>)> {
        let Some(viewer) = self
            .world_view()
            .viewer_of_owner(super::super::world_view::OwnerGuid(owner_guid))
        else {
            return Ok((Vec::new(), Vec::new()));
        };
        Ok((viewer.friend_guids(), viewer.ignored_guids()))
    }

    /// `owner_guid`'s ignore guids from this Shard's [`ContactIndex`], realm-wide safe for ANY
    /// owner, per [`crate::world::SocialStore::ignored_guids`]. Unlike `contact_lists`,
    /// `owner_guid` here is a Character this Gateway process may never have a `Viewer` for at all (a
    /// whisper sender or a guild-invite target is usually a PEER, not the connected session), so
    /// this cannot route through one.
    fn ignored_guids(&self, owner_guid: u64) -> Result<Vec<u64>> {
        let guard = self.0.coord();
        let contacts = guard.contacts.read().unwrap_or_else(|p| p.into_inner());
        Ok(contacts.ignored_by(owner_guid))
    }

    /// Resolve a typed name to a Character guid on this Shard, ASCII case-insensitive like the
    /// Module's own `helpers::character_by_name`, from the Shard's [`CharacterNameIndex`]. `None`
    /// if no Character here has that name. Every realm-wide name lookup (whisper, party and guild
    /// invites, contacts, channel moderation) reaches this through `presence::resolve_by_name` or
    /// `presence::resolve_all_by_name`.
    fn character_guid_by_name(&self, name: &str) -> Result<Option<u64>> {
        let guard = self.0.coord();
        let names = guard
            .character_names
            .read()
            .unwrap_or_else(|p| p.into_inner());
        Ok(names.guid_named(name))
    }

    /// `CMSG_ADD_FRIEND` — `target_guid` is already resolved by the gateway. `target_race` is the
    /// target's Speaker Fact, read realm-wide (`presence::of`): the Module's Enemy Gate needs it.
    fn add_friend(
        &self,
        actor: Actor,
        target_guid: u64,
        target_race: u8,
    ) -> Result<ContactOutcome> {
        contact_outcome(call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_add_friend",
            gw_add_friend_then(self.session_actor(actor), target_guid, target_race)
        ))
    }

    fn del_friend(&self, actor: Actor, target_guid: u64) -> Result<ContactOutcome> {
        contact_outcome(call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_del_friend",
            gw_del_friend_then(self.session_actor(actor), target_guid)
        ))
    }

    /// `CMSG_ADD_IGNORE` — `target_guid` is already resolved by the gateway.
    fn add_ignore(&self, actor: Actor, target_guid: u64) -> Result<ContactOutcome> {
        contact_outcome(call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_add_ignore",
            gw_add_ignore_then(self.session_actor(actor), target_guid)
        ))
    }

    fn del_ignore(&self, actor: Actor, target_guid: u64) -> Result<ContactOutcome> {
        contact_outcome(call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_del_ignore",
            gw_del_ignore_then(self.session_actor(actor), target_guid)
        ))
    }
}

/// The friends and ignore lists' Refusal decoder: a tagged Refusal is an outcome the client
/// renders, and an untagged one or a Transport Loss stays `Err`.
fn contact_outcome(result: Result<()>) -> Result<ContactOutcome> {
    let Err(error) = result else {
        return Ok(ContactOutcome::Done);
    };
    match classify(&error) {
        DurableFailure::Refusal { reason } => match ContactRefusal::parse_tag(reason) {
            Some(refusal) => Ok(refusal.into()),
            None => Err(error),
        },
        DurableFailure::TransportLoss => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stdb::ReducerCallError;

    #[test]
    fn a_contact_refusal_is_an_outcome_and_a_transport_loss_is_an_error() {
        let refused = ReducerCallError::refused("gw_add_friend", ContactRefusal::ListFull.as_tag());
        assert_eq!(
            contact_outcome(Err(refused.into())).unwrap(),
            ContactOutcome::Refused(ContactRefusal::ListFull)
        );
        let lost = ReducerCallError::transport_lost("gw_add_friend");
        assert!(contact_outcome(Err(lost.into())).is_err());
    }
}
