//! `Coordinator`'s [`ChannelActionStore`] adapter.

use anyhow::Result;
use lyracore_shared::channel::{ChannelName, ChannelRefusal};

use crate::stdb::bindings::*;
use crate::stdb::connection::{call_reducer, reducer_refusal_reason};
use crate::stdb::Coordinator;
use crate::world::{
    resolve_online_character, whisper, Actor, ChannelActionStore, ChannelOutcome, ChannelRoster,
    ResolvedTarget,
};

impl ChannelActionStore for Coordinator {
    /// `realm_channel_op`: run one Chat Channel op on Realm-core, or on the one database of an
    /// unsharded Realm. The Module applies every channel rule. The actor's name stays here.
    fn channel_op(
        &self,
        actor: Actor,
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
            realm_channel_op_then(realm.session_actor(actor), op, request)
        ))
    }

    /// The channel `channel_name` names for `team` on Realm-core, with its members in join order
    /// and its owner's name from whichever World Shard holds the Character.
    fn channel_roster(&self, team: u32, channel_name: &str) -> Result<Option<ChannelRoster>> {
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

    fn online_character_by_name(&self, name: &str) -> Result<Option<ResolvedTarget>> {
        resolve_online_character(self, name)
    }

    fn ignores(&self, owner_guid: u64, other_guid: u64) -> Result<bool> {
        whisper::ignored_anywhere(self, owner_guid, other_guid)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stdb::{classify, DurableFailure, ReducerCallError};

    #[test]
    fn a_rejected_channel_tag_decodes_to_its_refusal() {
        let error = ReducerCallError::refused("realm_channel_op", ChannelRefusal::Muted.as_tag());
        assert_eq!(
            channel_outcome(Err(error.into())).unwrap(),
            ChannelOutcome::Refused(ChannelRefusal::Muted)
        );
    }

    #[test]
    fn a_done_op_decodes_to_done() {
        assert_eq!(channel_outcome(Ok(())).unwrap(), ChannelOutcome::Done);
    }

    #[test]
    fn an_unknown_rejection_stays_a_refusal_for_the_handler() {
        let error = channel_outcome(Err(ReducerCallError::refused(
            "realm_channel_op",
            "mystery",
        )
        .into()))
        .unwrap_err();
        assert!(matches!(classify(&error), DurableFailure::Refusal { .. }));
    }

    #[test]
    fn a_lost_transport_stays_a_transport_loss() {
        let error = channel_outcome(Err(
            ReducerCallError::transport_lost("realm_channel_op").into()
        ))
        .unwrap_err();
        assert_eq!(classify(&error), DurableFailure::TransportLoss);
    }
}
