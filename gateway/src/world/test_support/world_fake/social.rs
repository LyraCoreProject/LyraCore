use super::super::*;

#[derive(Default)]
pub(crate) struct SocialState {
    /// Friend/ignore rows: `(owner_guid, target_guid, is_ignore)`. `add_friend`/
    /// `add_ignore`/`del_friend`/`del_ignore` mutate it; `contact_lists` reads it scoped to the caller.
    pub(crate) contacts: std::sync::Mutex<Vec<(u64, u64, bool)>>,
    /// This Shard's stored Auto-Replies, by Character guid.
    pub(crate) auto_replies: std::sync::Mutex<std::collections::HashMap<u64, String>>,
    /// Seeded characters that are nevertheless OFFLINE, so the invite gate's "player not
    /// online" arm can be driven. Empty = every seeded character is online, as before.
    pub(crate) offline_guids: Vec<u64>,
    /// Raw `PLAYER_FLAGS` per guid, for `presence_row`'s Away Status. Empty = every guid reads
    /// `AwayStatus::None`, as a Character with no live entity does in production.
    pub(crate) away_flags: std::collections::HashMap<u64, u32>,
    /// `game_area.name` per zone id, for `/who`'s search-string match. Empty = every zone name
    /// reads "", the "unimported catalogue" case.
    pub(crate) zone_names: std::collections::HashMap<u32, String>,
    /// When set, `contact_lists` fails with this message on THIS shard — the
    /// unreachable-database arm of the realm-wide ignore-list union.
    pub(crate) contact_lists_error: Option<String>,
    /// Live `game_world_entity` rows on THIS shard, as the columns Member Stats read.
    pub(crate) member_entities: std::sync::Mutex<Vec<(u64, codec::MemberEntity)>>,
    /// Characters THIS shard shows between two places: an online Session with no entity, or a
    /// bot named by a Transfer Intent.
    pub(crate) members_between_places: std::sync::Mutex<Vec<u64>>,
}

/// [`faked_party`] for the friends and ignore lists.
pub(crate) fn faked_contact(operation: &str, error: &str) -> Result<ContactOutcome> {
    match lyracore_shared::social::ContactRefusal::parse_tag(error) {
        Some(refusal) => Ok(refusal.into()),
        None => Err(crate::stdb::ReducerCallError::transport_lost(operation).into()),
    }
}

impl SocialStore for WorldFake {
    fn character_identity(&self, guid: u64) -> Result<Option<presence::CharacterIdentity>> {
        Ok(self.characters.iter().find(|c| c.guid == guid).map(|c| {
            presence::CharacterIdentity {
                guid: c.guid,
                name: c.name.clone(),
                race: c.race,
                class: c.class,
                level: c.level,
                zone_id: c.zone_id,
                // `offline_guids` drives the invite gate's "player not online" arm; a seeded
                // character is session-online unless listed there, mirroring `character_presence`.
                session_online: !self.social.offline_guids.contains(&guid),
            }
        }))
    }

    fn live_entity(&self, guid: u64) -> Option<codec::MemberEntity> {
        // `member_entities` alone: Member Stats' own tests despawn a guid here while it stays in
        // `live_guids` (a party-eligibility signal, not a Member Stats one) to pin the case where a
        // group mate's entity is gone but the party frame's own bookkeeping has not caught up —
        // falling back to `live_guids` or the blanket `entity_in_world` flag would read that guid
        // live again and silently defeat the pin. `in_world_players`'s bulk /who scan has its own,
        // separate fallback for a guid this fixture never gave a precise entity to.
        self.social
            .member_entities
            .lock()
            .unwrap()
            .iter()
            .find(|(g, _)| *g == guid)
            .map(|(_, e)| e.clone())
    }

    fn character_in_transit(&self, guid: u64) -> bool {
        self.social
            .members_between_places
            .lock()
            .unwrap()
            .contains(&guid)
    }

    fn auto_reply_text(&self, guid: u64) -> Result<Option<String>> {
        Ok(self.social.auto_replies.lock().unwrap().get(&guid).cloned())
    }

    fn every_shard_vouches_for_absence(&self) -> Result<()> {
        // A Realm Presence "gone" claim spans every configured Shard, not just this handle — each
        // Fake instance models one Shard's own connection, so the peer set is checked too, the
        // same reach `Coordinator::world_shards_for_absence` has from any one of its own handles.
        for peer in self.topology.peers.lock().unwrap().iter() {
            if let Some(error) = &peer.topology.world_shard_set_error {
                return Err(anyhow!(error.clone()));
            }
        }
        if let Some(error) = &self.topology.world_shard_set_error {
            return Err(anyhow!(error.clone()));
        }
        Ok(())
    }

    fn in_world_players(&self) -> Result<Vec<presence::RealmPresence>> {
        // Test store: every seeded character the fake considers in-world (`entity_in_world`,
        // the BLANKET flag included) is listed, so CMSG_WHO tests can assert a response without
        // wiring `live_guids` by hand — unlike `live_entity`, which `presence::of` uses for one
        // guid at a time and which deliberately does not trust that blanket flag.
        // `live_entity` supplies level/zone from `member_entities`/`live_guids` when a test seeded
        // one for this guid, else the durable row stands in.
        Ok(self
            .characters
            .iter()
            .filter(|c| self.entity_in_world(c.guid))
            .map(|c| {
                let entity = self.live_entity(c.guid).unwrap_or(codec::MemberEntity {
                    level: u32::from(c.level),
                    zone_id: c.zone_id,
                    player_flags: self.social.away_flags.get(&c.guid).copied().unwrap_or(0),
                    ..Default::default()
                });
                presence::RealmPresence {
                    guid: c.guid,
                    name: c.name.clone(),
                    race: c.race,
                    class: c.class,
                    level: u8::try_from(entity.level).unwrap_or(u8::MAX),
                    zone_id: entity.zone_id,
                    session_online: !self.social.offline_guids.contains(&c.guid),
                    whereabouts: presence::Whereabouts::InWorld {
                        away: self.away(c.guid),
                        entity,
                        shard_name: self.topology.shard.clone(),
                    },
                }
            })
            .collect())
    }

    fn zone_name(&self, zone_id: u32) -> String {
        self.social
            .zone_names
            .get(&zone_id)
            .cloned()
            .unwrap_or_default()
    }

    fn contact_lists(&self, self_guid: u64) -> Result<(Vec<u64>, Vec<u64>)> {
        if let Some(e) = &self.social.contact_lists_error {
            return Err(anyhow!("{e}"));
        }
        let contacts = self.social.contacts.lock().unwrap();
        let mut friends = Vec::new();
        let mut ignored = Vec::new();
        for &(owner, target, is_ignore) in contacts.iter() {
            if owner != self_guid {
                continue;
            }
            if is_ignore {
                ignored.push(target);
            } else {
                friends.push(target);
            }
        }
        Ok((friends, ignored))
    }

    fn ignored_guids(&self, owner_guid: u64) -> Result<Vec<u64>> {
        if let Some(e) = &self.social.contact_lists_error {
            return Err(anyhow!("{e}"));
        }
        Ok(self
            .social
            .contacts
            .lock()
            .unwrap()
            .iter()
            .filter(|&&(owner, _, is_ignore)| owner == owner_guid && is_ignore)
            .map(|&(_, target, _)| target)
            .collect())
    }

    fn character_guid_by_name(&self, name: &str) -> Result<Option<u64>> {
        Ok(self
            .characters
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
            .map(|c| c.guid))
    }

    fn add_friend(
        &self,
        _actor: Actor,
        target_guid: u64,
        _target_race: u8,
    ) -> Result<ContactOutcome> {
        if let Some(e) = &self.trade_error {
            return faked_contact("gw_add_friend", e);
        }
        let owner = self
            .session
            .login_entity
            .as_ref()
            .map(|e| e.guid)
            .unwrap_or(0);
        self.social
            .contacts
            .lock()
            .unwrap()
            .push((owner, target_guid, false));
        Ok(ContactOutcome::Done)
    }

    fn del_friend(&self, _actor: Actor, target_guid: u64) -> Result<ContactOutcome> {
        self.remove_contact(target_guid, false)
    }

    fn add_ignore(&self, _actor: Actor, target_guid: u64) -> Result<ContactOutcome> {
        if let Some(e) = &self.trade_error {
            return faked_contact("gw_add_ignore", e);
        }
        let owner = self
            .session
            .login_entity
            .as_ref()
            .map(|e| e.guid)
            .unwrap_or(0);
        self.social
            .contacts
            .lock()
            .unwrap()
            .push((owner, target_guid, true));
        Ok(ContactOutcome::Done)
    }

    fn del_ignore(&self, _actor: Actor, target_guid: u64) -> Result<ContactOutcome> {
        self.remove_contact(target_guid, true)
    }
}

impl WorldFake {
    /// Drop one contact row, or refuse when the owner does not hold it.
    pub(crate) fn remove_contact(
        &self,
        target_guid: u64,
        is_ignore: bool,
    ) -> Result<ContactOutcome> {
        let owner = self
            .session
            .login_entity
            .as_ref()
            .map(|e| e.guid)
            .unwrap_or(0);
        let mut contacts = self.social.contacts.lock().unwrap();
        let before = contacts.len();
        contacts.retain(|&(o, t, ig)| !(o == owner && t == target_guid && ig == is_ignore));
        if contacts.len() == before {
            return Ok(lyracore_shared::social::ContactRefusal::NotOnList.into());
        }
        Ok(ContactOutcome::Done)
    }

    /// `guid`'s Away Status from `away_flags` (raw `PLAYER_FLAGS`), `AwayStatus::None` when unset —
    /// mirroring "a Character with no live entity has `AwayStatus::None`" for every guid a test
    /// never seeds.
    pub(crate) fn away(&self, guid: u64) -> presence::AwayStatus {
        self.social
            .away_flags
            .get(&guid)
            .copied()
            .map(presence::away_from_player_flags)
            .unwrap_or(presence::AwayStatus::None)
    }
}
