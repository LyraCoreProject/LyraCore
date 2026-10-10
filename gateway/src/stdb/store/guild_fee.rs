//! `Coordinator`'s [`GuildFeeStore`] adapter.

use anyhow::{anyhow, Result};
use lyracore_shared::guild::GuildRefusal;

use crate::stdb::bindings::*;
use crate::stdb::connection::{call_reducer, reducer_refusal_reason};
use crate::stdb::reducers::{next_operation_id, wait_for_cache_row};
use crate::stdb::Coordinator;
use crate::world::guild_fee;
use crate::world::guild_fee::{
    CharterOwner, FeeHold, FeeOutcome, FeeRequest, FeeTerms, GuildFeeStore,
};

impl GuildFeeStore for crate::stdb::Coordinator {
    fn guild_fee_held(&self, actor_guid: u64) -> Result<Option<FeeHold>> {
        Ok(self.guild_fee_hold_row(actor_guid))
    }

    fn guild_fee_hold(
        &self,
        actor_guid: u64,
        request: FeeRequest,
    ) -> Result<Result<FeeHold, GuildRefusal>> {
        self.hold_guild_fee(actor_guid, request)
    }

    fn guild_fee_decide(&self, actor_guid: u64, hold: FeeHold) -> Result<FeeOutcome> {
        let owner = match hold.terms {
            FeeTerms::Emblem(_) => None,
            FeeTerms::Charter { .. } => {
                let facts = crate::stdb::Coordinator::guild_character_facts(self, actor_guid)?
                    .ok_or_else(|| anyhow!("Guild Charter owner {actor_guid} is unreadable"))?;
                Some(CharterOwner {
                    name: facts.name,
                    team: lyracore_shared::faction::team_for_race(facts.race),
                })
            }
        };
        self.realm_core()?.decide_guild_fee(actor_guid, hold, owner)
    }

    fn guild_fee_finish(&self, actor_guid: u64, operation_id: u64, accepted: bool) -> Result<()> {
        self.finish_guild_fee(actor_guid, operation_id, accepted)
    }
}

impl Coordinator {
    /// The Fee Hold of `payer_guid` in THIS handle's cache. Call it on the payer's Home Shard. A
    /// kind this Gateway does not know reads as none.
    pub(crate) fn guild_fee_hold_row(&self, payer_guid: u64) -> Option<guild_fee::FeeHold> {
        let hold = self
            .0
            .coord()
            .conn
            .db
            .game_guild_fee_hold()
            .payer_guid()
            .find(&payer_guid)?;
        let terms = match hold.kind {
            lyracore_shared::guild::fee_kind::EMBLEM => {
                guild_fee::FeeTerms::Emblem(guild_fee::Emblem {
                    emblem_style: hold.emblem_style,
                    emblem_color: hold.emblem_color,
                    border_style: hold.border_style,
                    border_color: hold.border_color,
                    background_color: hold.background_color,
                })
            }
            lyracore_shared::guild::fee_kind::CHARTER => guild_fee::FeeTerms::Charter {
                charter_item_guid: hold.charter_item_guid,
                name: hold.charter_name,
            },
            _ => return None,
        };
        Some(guild_fee::FeeHold {
            operation_id: hold.operation_id,
            terms,
        })
    }

    /// Realm-core's fee decision for `operation_id` in THIS handle's cache. Call it on the
    /// Realm-core handle.
    pub(crate) fn guild_fee_decision_row(&self, operation_id: u64) -> Option<GuildFeeDecision> {
        self.0
            .coord()
            .conn
            .db
            .game_guild_fee_decision()
            .operation_id()
            .find(&operation_id)
    }

    /// `gw_guild_fee_hold` on THIS handle, the actor's Home Shard, under a fresh operation id, then
    /// the Hold once this handle's cache shows it: the hold mints a Guild Charter's guid. A tagged
    /// Refusal answers `Ok(Err(_))`. The call is made once per operation id and never retried.
    pub(crate) fn hold_guild_fee(
        &self,
        actor_guid: u64,
        request: guild_fee::FeeRequest,
    ) -> Result<Result<guild_fee::FeeHold, GuildRefusal>> {
        let operation_id = next_operation_id()?;
        let request = match request {
            guild_fee::FeeRequest::Emblem { npc_guid, emblem } => {
                GuildFeeRequest::Emblem(GuildEmblemPurchase {
                    npc_guid,
                    emblem: guild_emblem(emblem),
                })
            }
            guild_fee::FeeRequest::Charter { npc_guid, name } => {
                GuildFeeRequest::Charter(GuildCharterPurchase { npc_guid, name })
            }
        };
        let result = call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_guild_fee_hold",
            gw_guild_fee_hold_then(operation_id, self.actor_or_owner(actor_guid), request)
        );
        match result {
            Ok(()) => wait_for_cache_row(operation_id, "guild Fee Hold", || {
                self.guild_fee_hold_row(actor_guid)
                    .filter(|hold| hold.operation_id == operation_id)
            })
            .map(Ok),
            Err(error) => match reducer_refusal_reason(&error).and_then(GuildRefusal::parse_tag) {
                Some(refusal) => Ok(Err(refusal)),
                None => Err(error),
            },
        }
    }

    /// `realm_guild_fee_decide` on THIS handle, Realm-core, then the decision once this handle's
    /// cache shows it. The reducer is idempotent, so a re-drive calls it again. A Charter needs
    /// `owner`, the facts Realm-core cannot read.
    pub(crate) fn decide_guild_fee(
        &self,
        actor_guid: u64,
        hold: guild_fee::FeeHold,
        owner: Option<guild_fee::CharterOwner>,
    ) -> Result<guild_fee::FeeOutcome> {
        let terms = match (hold.terms, owner) {
            (guild_fee::FeeTerms::Emblem(emblem), _) => GuildFeeTerms::Emblem(guild_emblem(emblem)),
            (
                guild_fee::FeeTerms::Charter {
                    charter_item_guid,
                    name,
                },
                Some(owner),
            ) => GuildFeeTerms::Charter(GuildCharterTerms {
                charter_item_guid,
                name,
                payer_name: owner.name,
                payer_team: owner.team,
            }),
            (guild_fee::FeeTerms::Charter { .. }, None) => {
                return Err(anyhow!(
                    "Guild Charter decision {} has no owner facts",
                    hold.operation_id
                ));
            }
        };
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_guild_fee_decide",
            realm_guild_fee_decide_then(hold.operation_id, self.actor_or_owner(actor_guid), terms)
        )?;
        let decision = wait_for_cache_row(hold.operation_id, "guild fee decision", || {
            self.guild_fee_decision_row(hold.operation_id)
        })?;
        fee_outcome(&decision)
    }

    /// `gw_guild_fee_finish` on THIS handle, the actor's Home Shard.
    pub(crate) fn finish_guild_fee(
        &self,
        actor_guid: u64,
        operation_id: u64,
        accepted: bool,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_guild_fee_finish",
            gw_guild_fee_finish_then(operation_id, self.actor_or_owner(actor_guid), accepted)
        )
    }
}

fn guild_emblem(emblem: guild_fee::Emblem) -> GuildEmblem {
    GuildEmblem {
        emblem_style: emblem.emblem_style,
        emblem_color: emblem.emblem_color,
        border_style: emblem.border_style,
        border_color: emblem.border_color,
        background_color: emblem.background_color,
    }
}

/// Realm-core's decision row as a fee outcome. A refusal tag this Gateway does not know is an
/// error, so the Hold stays for a Gateway that does.
fn fee_outcome(decision: &GuildFeeDecision) -> Result<guild_fee::FeeOutcome> {
    if decision.accepted {
        return Ok(guild_fee::FeeOutcome::Accepted);
    }
    GuildRefusal::parse_tag(&decision.refusal)
        .map(guild_fee::FeeOutcome::Refused)
        .ok_or_else(|| {
            anyhow!(
                "guild fee decision {} carries an unknown refusal {:?}",
                decision.operation_id,
                decision.refusal
            )
        })
}

#[cfg(test)]
mod guild_fee_reducer_tests {
    use super::*;

    fn decision(accepted: bool, refusal: &str) -> GuildFeeDecision {
        GuildFeeDecision {
            operation_id: 9,
            payer_guid: 5_090_401,
            kind: lyracore_shared::guild::fee_kind::EMBLEM,
            accepted,
            refusal: refusal.into(),
            petition_id: 0,
            decided_micros: 0,
        }
    }

    #[test]
    fn a_decision_row_maps_to_its_fee_outcome() {
        assert_eq!(
            fee_outcome(&decision(true, "")).unwrap(),
            guild_fee::FeeOutcome::Accepted
        );
        assert_eq!(
            fee_outcome(&decision(false, "guild:not_leader")).unwrap(),
            guild_fee::FeeOutcome::Refused(GuildRefusal::NotLeader)
        );
        assert!(
            fee_outcome(&decision(false, "guild:from_a_newer_module")).is_err(),
            "an unknown refusal must not read as a spend or a refund"
        );
    }
}
