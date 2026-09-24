//! The Gateway half of the Fee Hold. It puts the three Durable Requests of a guild fee in order:
//! hold on the actor's Home Shard, decide on Realm-core, finish on the Home Shard. Every Gate and
//! the exactly-once rules live in the Module (`module/src/guild/fee.rs`).
//!
//! Recovery is forward only. A Hold that a crash or a lost connection left behind is decided from
//! its own terms and finished, at the next world entry and before the next fee. It is never
//! dropped. A Character has one Fee Hold at a time, on whichever Shard holds the Character.
//!
//! A hold call mints its own operation id and is never retried, so no Hold is taken twice for one
//! id. After a lost answer the Hold is found by its payer and re-driven, not held again.

use anyhow::{anyhow, Result};
use lyracore_shared::guild::GuildRefusal;

/// The five tabard design values of a Guild Emblem, in wire order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Emblem {
    pub(crate) emblem_style: u32,
    pub(crate) emblem_color: u32,
    pub(crate) border_style: u32,
    pub(crate) border_color: u32,
    pub(crate) background_color: u32,
}

/// What the actor asks to pay for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FeeRequest {
    /// Save `emblem` at the Tabard Designer `npc_guid`.
    Emblem { npc_guid: u64, emblem: Emblem },
    /// Buy a Guild Charter for a Guild named `name` at the Petitioner `npc_guid`.
    Charter { npc_guid: u64, name: String },
}

/// What Realm-core decides on, as a Fee Hold records it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FeeTerms {
    Emblem(Emblem),
    /// The Guild Charter the Hold created.
    Charter {
        charter_item_guid: u64,
        name: String,
    },
}

/// The owner facts a Charter decision carries. Realm-core holds no Character rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CharterOwner {
    pub(crate) name: String,
    /// `lyracore_shared::faction::TEAM_*`.
    pub(crate) team: u32,
}

/// One Fee Hold on the actor's Home Shard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FeeHold {
    pub(crate) operation_id: u64,
    pub(crate) terms: FeeTerms,
}

/// Realm-core's decision on a guild fee.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FeeOutcome {
    Accepted,
    Refused(GuildRefusal),
}

/// The Durable Requests and reads of a guild fee. `self` is the actor's Home Shard; decisions go
/// to Realm-core.
pub(crate) trait GuildFeeStore: Send + Sync {
    /// The actor's Fee Hold on its Home Shard, if one is left.
    fn guild_fee_held(&self, actor_guid: u64) -> Result<Option<FeeHold>>;
    /// Move the fee for `request` from the actor's purse into a new Fee Hold under a fresh
    /// operation id. `Ok(Err(_))` is a hold Refusal, and nothing changed.
    fn guild_fee_hold(
        &self,
        actor_guid: u64,
        request: FeeRequest,
    ) -> Result<Result<FeeHold, GuildRefusal>>;
    /// Realm-core's one decision for `hold`, committed now unless it already exists.
    fn guild_fee_decide(&self, actor_guid: u64, hold: FeeHold) -> Result<FeeOutcome>;
    /// Spend the Hold when `accepted`, refund it otherwise, then delete it. Without a Hold for
    /// `operation_id` this changes nothing.
    fn guild_fee_finish(&self, actor_guid: u64, operation_id: u64, accepted: bool) -> Result<()>;
}

/// Pay a guild fee. A leftover Hold is finished first. A hold Refusal (the NPC, the purse)
/// returns before any copper moved. Any other answer is Realm-core's decision, and the Hold has
/// been spent or refunded to match it. `Err` leaves any Hold for the next re-drive.
pub(crate) fn pay<St: GuildFeeStore + ?Sized>(
    store: &St,
    actor_guid: u64,
    request: FeeRequest,
) -> Result<FeeOutcome> {
    finish_leftover(store, actor_guid)?;
    match store.guild_fee_hold(actor_guid, request)? {
        Ok(hold) => settle(store, actor_guid, hold),
        Err(refusal) => Ok(FeeOutcome::Refused(refusal)),
    }
}

/// Finish the actor's leftover Fee Hold at world entry. A failure logs and leaves the Hold for
/// the next attempt.
pub(crate) fn redrive<St: GuildFeeStore + ?Sized>(store: &St, actor_guid: u64) {
    if let Err(error) = finish_leftover(store, actor_guid) {
        log::warn!("world: Fee Hold of {actor_guid} left for the next re-drive: {error:#}");
    }
}

fn finish_leftover<St: GuildFeeStore + ?Sized>(store: &St, actor_guid: u64) -> Result<()> {
    let Some(hold) = store.guild_fee_held(actor_guid)? else {
        return Ok(());
    };
    let operation_id = hold.operation_id;
    let outcome = settle(store, actor_guid, hold)?;
    log::info!("world: finished leftover Fee Hold {operation_id} of {actor_guid}: {outcome:?}");
    Ok(())
}

fn settle<St: GuildFeeStore + ?Sized>(
    store: &St,
    actor_guid: u64,
    hold: FeeHold,
) -> Result<FeeOutcome> {
    let operation_id = hold.operation_id;
    let outcome = store.guild_fee_decide(actor_guid, hold)?;
    store.guild_fee_finish(actor_guid, operation_id, outcome == FeeOutcome::Accepted)?;
    Ok(outcome)
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use lyracore_shared::guild::EMBLEM_COST_COPPER;
    use std::sync::{Arc, Mutex};

    const LEADER: u64 = 5_090_401;
    const MEMBER: u64 = 5_090_402;
    const DESIGNER: u64 = 5_090_410;

    const EMBLEM: Emblem = Emblem {
        emblem_style: 11,
        emblem_color: 12,
        border_style: 3,
        border_color: 14,
        background_color: 15,
    };

    fn purchase() -> FeeRequest {
        FeeRequest::Emblem {
            npc_guid: DESIGNER,
            emblem: EMBLEM,
        }
    }

    /// The Durable Request after which the Gateway loses its answer: the reducer committed, the
    /// Gateway heard an error.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Step {
        Hold,
        Decide,
        Finish,
    }

    /// Realm-core, shared by every Home Shard of the realm.
    #[derive(Default)]
    struct Realm {
        leader: u64,
        emblem: Option<Emblem>,
        decisions: Vec<(u64, FeeOutcome)>,
        emblems_saved: usize,
    }

    /// One Home Shard: purses, the Fee Holds keyed by payer, and a step that fails once after
    /// committing.
    struct Shard {
        realm: Arc<Mutex<Realm>>,
        purses: Mutex<Vec<(u64, u32)>>,
        holds: Mutex<Vec<(u64, FeeHold, u32)>>,
        crash_after: Mutex<Option<Step>>,
        next_operation_id: Mutex<u64>,
    }

    impl Shard {
        fn new(realm: &Arc<Mutex<Realm>>, purse: u32) -> Self {
            Self {
                realm: realm.clone(),
                purses: Mutex::new(vec![(LEADER, purse), (MEMBER, purse)]),
                holds: Mutex::new(Vec::new()),
                crash_after: Mutex::new(None),
                next_operation_id: Mutex::new(1),
            }
        }

        fn purse(&self, guid: u64) -> u32 {
            self.purses
                .lock()
                .unwrap()
                .iter()
                .find(|(payer, _)| *payer == guid)
                .map_or(0, |(_, copper)| *copper)
        }

        fn credit(&self, guid: u64, copper: i64) {
            let mut purses = self.purses.lock().unwrap();
            let purse = &mut purses
                .iter_mut()
                .find(|(payer, _)| *payer == guid)
                .unwrap()
                .1;
            *purse = u32::try_from(i64::from(*purse) + copper).unwrap();
        }

        fn crash_after(&self, step: Step) {
            *self.crash_after.lock().unwrap() = Some(step);
        }

        fn committed(&self, step: Step) -> Result<()> {
            let mut crash = self.crash_after.lock().unwrap();
            if *crash == Some(step) {
                *crash = None;
                return Err(anyhow!("connection lost after {step:?} committed"));
            }
            Ok(())
        }

        /// Move the actor's Hold to `other`, as a Transfer carries it with the Character.
        fn transfer_holds_to(&self, other: &Shard) {
            other
                .holds
                .lock()
                .unwrap()
                .append(&mut self.holds.lock().unwrap());
        }
    }

    impl GuildFeeStore for Shard {
        fn guild_fee_held(&self, actor_guid: u64) -> Result<Option<FeeHold>> {
            Ok(self
                .holds
                .lock()
                .unwrap()
                .iter()
                .find(|(payer, _, _)| *payer == actor_guid)
                .map(|(_, hold, _)| hold.clone()))
        }

        fn guild_fee_hold(
            &self,
            actor_guid: u64,
            request: FeeRequest,
        ) -> Result<Result<FeeHold, GuildRefusal>> {
            if self.guild_fee_held(actor_guid)?.is_some() {
                return Err(anyhow!("guild fee: another Fee Hold is pending"));
            }
            if self.purse(actor_guid) < EMBLEM_COST_COPPER {
                return Ok(Err(GuildRefusal::NotEnoughMoney));
            }
            let FeeRequest::Emblem { emblem, .. } = request else {
                panic!("these tests pay for emblems");
            };
            let mut next = self.next_operation_id.lock().unwrap();
            let hold = FeeHold {
                operation_id: *next,
                terms: FeeTerms::Emblem(emblem),
            };
            *next += 1;
            self.credit(actor_guid, -i64::from(EMBLEM_COST_COPPER));
            self.holds
                .lock()
                .unwrap()
                .push((actor_guid, hold.clone(), EMBLEM_COST_COPPER));
            self.committed(Step::Hold)?;
            Ok(Ok(hold))
        }

        fn guild_fee_decide(&self, actor_guid: u64, hold: FeeHold) -> Result<FeeOutcome> {
            let mut realm = self.realm.lock().unwrap();
            let known = realm
                .decisions
                .iter()
                .find(|(id, _)| *id == hold.operation_id)
                .map(|(_, outcome)| *outcome);
            let outcome = match known {
                Some(outcome) => outcome,
                None => {
                    let outcome = if realm.leader == actor_guid {
                        let FeeTerms::Emblem(emblem) = hold.terms else {
                            panic!("these tests pay for emblems");
                        };
                        realm.emblem = Some(emblem);
                        realm.emblems_saved += 1;
                        FeeOutcome::Accepted
                    } else {
                        FeeOutcome::Refused(GuildRefusal::NotLeader)
                    };
                    realm.decisions.push((hold.operation_id, outcome));
                    outcome
                }
            };
            drop(realm);
            self.committed(Step::Decide)?;
            Ok(outcome)
        }

        fn guild_fee_finish(
            &self,
            actor_guid: u64,
            operation_id: u64,
            accepted: bool,
        ) -> Result<()> {
            let mut holds = self.holds.lock().unwrap();
            if let Some(at) = holds.iter().position(|(payer, hold, _)| {
                *payer == actor_guid && hold.operation_id == operation_id
            }) {
                let (_, _, copper) = holds.remove(at);
                drop(holds);
                if !accepted {
                    self.credit(actor_guid, i64::from(copper));
                }
            }
            self.committed(Step::Finish)
        }
    }

    fn realm_led_by(leader: u64) -> Arc<Mutex<Realm>> {
        Arc::new(Mutex::new(Realm {
            leader,
            ..Realm::default()
        }))
    }

    #[test]
    fn the_leader_pays_ten_gold_for_the_emblem() {
        let realm = realm_led_by(LEADER);
        let home = Shard::new(&realm, 150_000);
        assert_eq!(
            pay(&home, LEADER, purchase()).unwrap(),
            FeeOutcome::Accepted
        );
        assert_eq!(home.purse(LEADER), 50_000);
        assert_eq!(home.guild_fee_held(LEADER).unwrap(), None);
        assert_eq!(realm.lock().unwrap().emblem, Some(EMBLEM));
    }

    #[test]
    fn a_refused_fee_is_refunded_before_pay_returns() {
        let realm = realm_led_by(LEADER);
        let home = Shard::new(&realm, 150_000);
        assert_eq!(
            pay(&home, MEMBER, purchase()).unwrap(),
            FeeOutcome::Refused(GuildRefusal::NotLeader)
        );
        assert_eq!(home.purse(MEMBER), 150_000);
        assert_eq!(home.guild_fee_held(MEMBER).unwrap(), None);
        assert_eq!(realm.lock().unwrap().emblem, None);
    }

    #[test]
    fn a_hold_refusal_moves_nothing_and_asks_realm_core_nothing() {
        let realm = realm_led_by(LEADER);
        let home = Shard::new(&realm, 99_999);
        assert_eq!(
            pay(&home, LEADER, purchase()).unwrap(),
            FeeOutcome::Refused(GuildRefusal::NotEnoughMoney)
        );
        assert_eq!(home.purse(LEADER), 99_999);
        assert!(realm.lock().unwrap().decisions.is_empty());
    }

    /// A crash after each step leaves one Hold. The next world entry finishes it with exactly
    /// one outcome: the copper is spent or refunded once, and the Guild agrees.
    #[test]
    fn world_entry_finishes_a_hold_left_after_any_step_exactly_once() {
        for step in [Step::Hold, Step::Decide, Step::Finish] {
            for (payer, spent) in [(LEADER, true), (MEMBER, false)] {
                let realm = realm_led_by(LEADER);
                let home = Shard::new(&realm, 150_000);
                home.crash_after(step);
                assert!(pay(&home, payer, purchase()).is_err(), "{step:?}");

                redrive(&home, payer);
                redrive(&home, payer);

                let purse = if spent { 50_000 } else { 150_000 };
                assert_eq!(home.purse(payer), purse, "{step:?} {payer}");
                assert_eq!(home.guild_fee_held(payer).unwrap(), None, "{step:?}");
                let realm = realm.lock().unwrap();
                assert_eq!(realm.decisions.len(), 1, "{step:?}");
                assert_eq!(realm.emblems_saved, usize::from(spent), "{step:?}");
            }
        }
    }

    #[test]
    fn the_next_fee_finishes_a_leftover_hold_first() {
        let realm = realm_led_by(LEADER);
        let home = Shard::new(&realm, 250_000);
        home.crash_after(Step::Decide);
        assert!(pay(&home, LEADER, purchase()).is_err());
        assert_eq!(
            pay(&home, LEADER, purchase()).unwrap(),
            FeeOutcome::Accepted
        );
        assert_eq!(home.purse(LEADER), 50_000);
        assert_eq!(realm.lock().unwrap().emblems_saved, 2);
    }

    #[test]
    fn a_hold_that_crossed_a_shard_boundary_is_finished_on_the_new_home_shard() {
        let realm = realm_led_by(LEADER);
        let old_home = Shard::new(&realm, 150_000);
        let new_home = Shard::new(&realm, 0);
        old_home.crash_after(Step::Hold);
        assert!(pay(&old_home, MEMBER, purchase()).is_err());
        assert_eq!(old_home.purse(MEMBER), 50_000);

        old_home.transfer_holds_to(&new_home);
        new_home.credit(MEMBER, 50_000);
        redrive(&new_home, MEMBER);

        assert_eq!(new_home.purse(MEMBER), 150_000);
        assert_eq!(new_home.guild_fee_held(MEMBER).unwrap(), None);
        assert_eq!(old_home.guild_fee_held(MEMBER).unwrap(), None);
    }
}
