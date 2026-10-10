//! `Coordinator`'s [`ChannelActionStore`] adapter.

use anyhow::Result;
use lyracore_shared::channel::{ChannelName, ChannelRefusal};

use crate::stdb::bindings::*;
use crate::stdb::connection::{call_reducer, reducer_refusal_reason};
use crate::stdb::Coordinator;
use crate::world::{
    resolve_online_character, whisper, ChannelActionStore, ChannelOutcome, ChannelRoster,
    ResolvedTarget,
};

impl ChannelActionStore for crate::stdb::Coordinator {
    fn channel_op(
        &self,
        actor_guid: u64,
        op: u8,
        request: crate::world::ChannelRequest,
    ) -> Result<ChannelOutcome> {
        crate::stdb::Coordinator::channel_op(self, actor_guid, op, request)
    }

    fn channel_roster(&self, team: u32, channel_name: &str) -> Result<Option<ChannelRoster>> {
        crate::stdb::Coordinator::channel_roster(self, team, channel_name)
    }

    fn online_character_by_name(&self, name: &str) -> Result<Option<ResolvedTarget>> {
        resolve_online_character(self, name)
    }

    fn ignores(&self, owner_guid: u64, other_guid: u64) -> Result<bool> {
        whisper::ignored_anywhere(self, owner_guid, other_guid)
    }
}

impl Coordinator {
    /// The channel `channel_name` names for `team` on Realm-core, with its members in join order
    /// and its owner's name from whichever World Shard holds the Character.
    pub(crate) fn channel_roster(
        &self,
        team: u32,
        channel_name: &str,
    ) -> anyhow::Result<Option<ChannelRoster>> {
        let realm = self.realm_core()?;
        let key = ChannelName::normalize(channel_name).key;
        let found = {
            let guard = realm.0.coord();
            // The index lock is released before the cache read: the pump takes the cache and
            // then the index in its callbacks.
            let indexed = {
                let index = guard.chat_channels.read().unwrap();
                index
                    .channel_id(team, &key)
                    .map(|channel_id| (channel_id, index.members(channel_id)))
            };
            indexed.and_then(|(channel_id, members)| {
                let channel = guard
                    .conn
                    .db
                    .game_chat_channel()
                    .channel_id()
                    .find(&channel_id)?;
                Some((channel, members))
            })
        };
        // The guard is gone: the owner's name reads other Shards' caches.
        let Some((channel, members)) = found else {
            return Ok(None);
        };
        let owner_name = if channel.owner_guid == 0 {
            String::new()
        } else {
            crate::world::presence::character_anywhere(self, channel.owner_guid)?
                .map(|character| character.name)
                .unwrap_or_default()
        };
        Ok(Some(ChannelRoster {
            name: channel.name,
            flags: channel.flags,
            owner_guid: channel.owner_guid,
            owner_name,
            members,
        }))
    }

    /// `realm_channel_op`: run one Chat Channel op on Realm-core, or on the one database of an
    /// unsharded Realm. The Module applies every channel rule. The actor's name stays here.
    pub fn channel_op(
        &self,
        actor_guid: u64,
        op: u8,
        request: crate::world::ChannelRequest,
    ) -> Result<ChannelOutcome> {
        let realm = self.realm_core()?;
        let request = ChannelRequest {
            channel_name: request.channel_name,
            password: request.password,
            target_guid: request.target_guid,
            target_name: request.target_name,
            target_race: request.target_race,
            target_ignores_actor: request.target_ignores_actor,
            speaker: SpeakerFacts {
                race: request.speaker.race,
                chat_tag: request.speaker.chat_tag,
            },
        };
        channel_outcome(call_reducer!(
            realm.0.call_pipe().conn.reducers,
            "realm_channel_op",
            realm_channel_op_then(realm.actor_or_owner(actor_guid), op, request)
        ))
    }
}

/// The Module's typed channel Refusal. Only a reducer the Module rejected carries a tag; a timeout,
/// transport, or SDK failure stays an error with an unknown outcome.
fn channel_outcome(result: Result<()>) -> Result<ChannelOutcome> {
    match result {
        Ok(()) => Ok(ChannelOutcome::Done),
        Err(error) => match reducer_refusal_reason(&error).and_then(ChannelRefusal::parse_tag) {
            Some(refusal) => Ok(ChannelOutcome::Refused(refusal)),
            None => Err(error),
        },
    }
}
