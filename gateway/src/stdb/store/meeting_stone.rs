//! `Coordinator`'s [`MeetingStoneActionStore`] adapter.

use anyhow::Result;
use lyracore_shared::constants::go_type;
use lyracore_shared::meeting_stone::MeetingStoneRefusal;

use crate::stdb::bindings::*;
use crate::stdb::connection::{call_reducer, reducer_refusal_reason};
use crate::stdb::Coordinator;
use crate::world::{presence, Actor, MeetingStoneActionStore, MeetingStoneOutcome};

impl MeetingStoneActionStore for Coordinator {
    /// `gw_admit_meeting_stone` on this Home Shard: may the actor use the stone? Writes nothing.
    fn admit_meeting_stone(&self, actor: Actor, go_guid: u64) -> Result<MeetingStoneOutcome> {
        meeting_stone_outcome(call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_admit_meeting_stone",
            gw_admit_meeting_stone_then(self.session_actor(actor), go_guid)
        ))
    }

    /// The dungeon area of the Meeting Stone spawned as `go_guid` on this database, if any.
    fn meeting_stone_area(&self, go_guid: u64) -> Result<Option<u32>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        Ok(db
            .game_gameobject()
            .guid()
            .find(&go_guid)
            .and_then(|go| {
                db.game_gameobject_template()
                    .entry()
                    .find(&go.template_entry)
            })
            .filter(|template| template.type_id == go_type::MEETINGSTONE)
            .and_then(|template| db.game_meeting_stone().entry().find(&template.entry))
            .map(|stone| stone.area_id))
    }

    fn party_members(&self, actor_guid: u64) -> Result<Option<Vec<u64>>> {
        let authority = self.realm_core()?;
        Ok(
            crate::stdb::Coordinator::group_roster(&authority, actor_guid)
                .map(|roster| roster.members.iter().map(|member| member.guid).collect()),
        )
    }

    fn seeker_facts(&self, character_guid: u64) -> Result<Option<crate::world::SeekerFacts>> {
        Ok(
            presence::character_anywhere(self, character_guid)?.map(|character| {
                crate::world::SeekerFacts {
                    character_guid,
                    race: character.race,
                    class: character.class,
                }
            }),
        )
    }

    /// `realm_meeting_stone_op`: run one Meeting Stone Queue op on Realm-core, or on the one
    /// database of an unsharded Realm. The Module applies every queue rule.
    fn meeting_stone_op(
        &self,
        actor: Actor,
        op: u8,
        area_id: u32,
        seekers: Vec<crate::world::SeekerFacts>,
    ) -> Result<MeetingStoneOutcome> {
        let realm = self.realm_core()?;
        let seekers = seekers
            .into_iter()
            .map(|facts| SeekerFacts {
                character_guid: facts.character_guid,
                race: facts.race,
                class: facts.class,
            })
            .collect();
        meeting_stone_outcome(call_reducer!(
            realm.0.call_pipe().conn.reducers,
            "realm_meeting_stone_op",
            realm_meeting_stone_op_then(op, realm.session_actor(actor), area_id, seekers)
        ))
    }

    /// The area `character_guid` is queued for, read from the party authority.
    fn queued_area(&self, character_guid: u64) -> Result<Option<u32>> {
        let realm = self.realm_core()?;
        let guard = realm.0.coord();
        Ok(guard
            .conn
            .db
            .game_meeting_stone_seeker()
            .character_guid()
            .find(&character_guid)
            .map(|seeker| seeker.area_id))
    }
}

fn meeting_stone_outcome(result: Result<()>) -> Result<MeetingStoneOutcome> {
    match result {
        Ok(()) => Ok(MeetingStoneOutcome::Ran),
        Err(error) => match reducer_refusal_reason(&error).and_then(MeetingStoneRefusal::parse_tag)
        {
            Some(refusal) => Ok(MeetingStoneOutcome::Refused(refusal)),
            None => Err(error),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stdb::ReducerCallError;

    #[test]
    fn a_tagged_rejection_is_an_outcome_and_a_transport_loss_stays_an_error() {
        let refused = ReducerCallError::refused(
            "gw_admit_meeting_stone",
            MeetingStoneRefusal::PartyFull.as_tag(),
        );
        assert_eq!(
            meeting_stone_outcome(Err(refused.into())).unwrap(),
            MeetingStoneOutcome::Refused(MeetingStoneRefusal::PartyFull)
        );

        let lost = ReducerCallError::transport_lost("gw_admit_meeting_stone");
        assert!(meeting_stone_outcome(Err(lost.into())).is_err());

        assert_eq!(
            meeting_stone_outcome(Ok(())).unwrap(),
            MeetingStoneOutcome::Ran
        );
    }
}
