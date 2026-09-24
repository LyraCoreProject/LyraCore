//! The Fee Hold: how a guild operation that costs copper pays across two databases.
//!
//! The purse lives on the payer's Home Shard. Whether the operation is allowed is known only on
//! Realm-core. No transaction spans both, so one fee takes three Durable Requests, each idempotent
//! on the Gateway's operation id:
//!
//! 1. [`gw_guild_fee_hold`] on the Home Shard runs the NPC and purse Gates and moves the fee from
//!    the purse into the payer's Fee Hold.
//! 2. [`realm_guild_fee_decide`] on Realm-core runs the guild Gates and commits one decision row
//!    for the operation id, with the accepted effect in the same transaction. A refusal is data in
//!    that row, so a retry gets the same answer.
//! 3. [`gw_guild_fee_finish`] on the Home Shard spends the Hold on acceptance or puts the copper
//!    back on refusal, and deletes the Hold last.
//!
//! What the fee pays for can vanish between the phases: the Guild disbands, leadership moves. The
//! decision then refuses and the finish refunds, so copper never strands. A Hold that a crash left
//! behind travels with its Character on Transfer, and the Gateway finishes it at the next world
//! entry. Character deletion is refused while one exists.
//!
//! The Gateway calls hold at most once per operation id and always holds before it decides. A
//! finish or decide that arrives without its Hold spends nothing.

use lyracore_shared::constants::npc_flags;
use lyracore_shared::guild::{event_kind, fee_kind, GuildRefusal, EMBLEM_COST_COPPER};
use spacetimedb::{reducer, table, ReducerContext, SpacetimeType, Table};

use super::{game_guild, member, push_event};
use crate::game_world_entity;

/// Copper a Character paid for a guild operation that is not finished yet. Keyed by the payer, so
/// a Character has at most one Fee Hold and the Gateway finds it by guid. The row carries every
/// input the decision needs, so a re-drive decides from the Hold alone.
#[table(accessor = game_guild_fee_hold)]
pub struct GuildFeeHold {
    #[primary_key]
    pub payer_guid: u64,
    pub operation_id: u64,
    /// `lyracore_shared::guild::fee_kind`.
    pub kind: u8,
    pub copper: u32,
    /// The Tabard Designer or petitioner the payer stood at.
    pub npc_guid: u64,
    pub emblem_style: u32,
    pub emblem_color: u32,
    pub border_style: u32,
    pub border_color: u32,
    pub background_color: u32,
    /// The Guild Charter the Hold created. 0 for an emblem.
    pub charter_item_guid: u64,
    /// The guild name on the Guild Charter. Empty for an emblem.
    pub charter_name: String,
    pub created_micros: i64,
}
crate::character_owned!(delete, fn sweep_delete_game_guild_fee_hold(ctx, character_guid) {
    ctx.db.game_guild_fee_hold().payer_guid().delete(character_guid);
});
crate::character_owned!(transfer, fn sweep_transfer_game_guild_fee_hold(ctx, character_guid, io) {
    table = game_guild_fee_hold,
    primary_key = payer_guid,
});

/// Realm-core's one decision for one operation id. Kept after the Hold is finished: a guild pays
/// a few fees in its lifetime, and the row is what makes a late retry harmless.
#[table(accessor = game_guild_fee_decision)]
pub struct GuildFeeDecision {
    #[primary_key]
    pub operation_id: u64,
    pub payer_guid: u64,
    pub kind: u8,
    pub accepted: bool,
    /// The `GuildRefusal` tag when refused, empty when accepted.
    pub refusal: String,
    /// The petition a Guild Charter opened. 0 for an emblem.
    pub petition_id: u32,
    pub decided_micros: i64,
}

/// The five tabard design values of a Guild, in the order the client sends them.
#[derive(SpacetimeType, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GuildEmblem {
    pub emblem_style: u32,
    pub emblem_color: u32,
    pub border_style: u32,
    pub border_color: u32,
    pub background_color: u32,
}

/// A Guild Emblem bought at the Tabard Designer `npc_guid`.
#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub struct GuildEmblemPurchase {
    pub npc_guid: u64,
    pub emblem: GuildEmblem,
}

/// What the payer asks its Home Shard to hold copper for. Later fee kinds append variants.
#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub enum GuildFeeRequest {
    Emblem(GuildEmblemPurchase),
}

/// What Realm-core decides on: the operation a Fee Hold pays for, read back from the Hold.
#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub enum GuildFeeTerms {
    Emblem(GuildEmblem),
}

impl GuildFeeTerms {
    fn kind(&self) -> u8 {
        match self {
            Self::Emblem(_) => fee_kind::EMBLEM,
        }
    }

    fn copper(&self) -> u32 {
        match self {
            Self::Emblem(_) => EMBLEM_COST_COPPER,
        }
    }
}

/// One Fee Hold as the protocol sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct HeldFee {
    operation_id: u64,
    payer_guid: u64,
    copper: u32,
    npc_guid: u64,
    terms: GuildFeeTerms,
}

impl HeldFee {
    fn requested(operation_id: u64, payer_guid: u64, request: &GuildFeeRequest) -> Self {
        let (npc_guid, terms) = match request {
            GuildFeeRequest::Emblem(purchase) => {
                (purchase.npc_guid, GuildFeeTerms::Emblem(purchase.emblem))
            }
        };
        Self {
            operation_id,
            payer_guid,
            copper: terms.copper(),
            npc_guid,
            terms,
        }
    }

    fn from_row(row: GuildFeeHold) -> Option<Self> {
        let terms = match row.kind {
            fee_kind::EMBLEM => GuildFeeTerms::Emblem(GuildEmblem {
                emblem_style: row.emblem_style,
                emblem_color: row.emblem_color,
                border_style: row.border_style,
                border_color: row.border_color,
                background_color: row.background_color,
            }),
            _ => return None,
        };
        Some(Self {
            operation_id: row.operation_id,
            payer_guid: row.payer_guid,
            copper: row.copper,
            npc_guid: row.npc_guid,
            terms,
        })
    }

    fn into_row(self, created_micros: i64) -> GuildFeeHold {
        let kind = self.terms.kind();
        let GuildFeeTerms::Emblem(emblem) = self.terms;
        GuildFeeHold {
            payer_guid: self.payer_guid,
            operation_id: self.operation_id,
            kind,
            copper: self.copper,
            npc_guid: self.npc_guid,
            emblem_style: emblem.emblem_style,
            emblem_color: emblem.emblem_color,
            border_style: emblem.border_style,
            border_color: emblem.border_color,
            background_color: emblem.background_color,
            charter_item_guid: 0,
            charter_name: String::new(),
            created_micros,
        }
    }
}

/// Why a fee Durable Request did nothing. A Refusal answers the player; a conflict is a Gateway or
/// transport fault and leaves the Hold for the next re-drive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FeeError {
    Refused(GuildRefusal),
    Conflict(&'static str),
}

impl FeeError {
    fn into_text(self) -> String {
        match self {
            Self::Refused(refusal) => refusal.as_tag().to_string(),
            Self::Conflict(reason) => format!("guild fee: {reason}"),
        }
    }
}

/// The Home Shard half: the payer's purse and its Fee Hold.
trait FeePurse {
    fn held(&self, payer_guid: u64) -> Option<HeldFee>;
    /// The payer's purse, `None` when the payer is not in the world on this Shard.
    fn purse(&self, payer_guid: u64) -> Option<u32>;
    /// Does `npc_guid` serve fee `kind` to the payer here: flag, alive, in reach, not refusing?
    fn npc_serves(&self, payer_guid: u64, npc_guid: u64, kind: u8) -> bool;
    /// Take `hold.copper` from the purse and insert the Hold.
    fn take(&mut self, hold: HeldFee) -> Result<(), FeeError>;
    /// Put `refund` back in the purse and delete the Hold.
    fn release(&mut self, payer_guid: u64, refund: u32) -> Result<(), FeeError>;
}

/// The Realm-core half: guild facts and the decision ledger.
trait FeeLedger {
    /// The payer and kind a decision for `operation_id` was made for.
    fn decided_for(&self, operation_id: u64) -> Option<(u64, u8)>;
    /// The payer's Guild and its Guild Leader, when the payer is a member.
    fn guild_of(&self, payer_guid: u64) -> Option<(u32, u64)>;
    fn apply(&mut self, guild_id: u32, terms: &GuildFeeTerms);
    fn record(
        &mut self,
        operation_id: u64,
        payer_guid: u64,
        kind: u8,
        refusal: Option<GuildRefusal>,
    );
}

/// Phase 1. Gates in mangos order: the NPC, then the purse (`cm:GuildHandler.cpp:726-755`).
fn hold_fee<P: FeePurse>(purse: &mut P, hold: HeldFee) -> Result<(), FeeError> {
    if hold.operation_id == 0 {
        return Err(FeeError::Conflict("operation id 0"));
    }
    if let Some(existing) = purse.held(hold.payer_guid) {
        return if existing == hold {
            Ok(())
        } else {
            Err(FeeError::Conflict("another Fee Hold is pending"))
        };
    }
    if !purse.npc_serves(hold.payer_guid, hold.npc_guid, hold.terms.kind()) {
        return Err(FeeError::Refused(GuildRefusal::NpcRefused));
    }
    if purse
        .purse(hold.payer_guid)
        .is_none_or(|copper| copper < hold.copper)
    {
        return Err(FeeError::Refused(GuildRefusal::NotEnoughMoney));
    }
    purse.take(hold)
}

/// Phase 2. Exactly one decision per operation id; a retry changes nothing.
fn decide_fee<L: FeeLedger>(
    ledger: &mut L,
    operation_id: u64,
    payer_guid: u64,
    terms: &GuildFeeTerms,
) -> Result<(), FeeError> {
    if operation_id == 0 {
        return Err(FeeError::Conflict("operation id 0"));
    }
    if let Some(decided_for) = ledger.decided_for(operation_id) {
        return if decided_for == (payer_guid, terms.kind()) {
            Ok(())
        } else {
            Err(FeeError::Conflict("operation id reused"))
        };
    }
    let verdict = match terms {
        GuildFeeTerms::Emblem(_) => emblem_gate(ledger, payer_guid),
    };
    if let Ok(guild_id) = verdict {
        ledger.apply(guild_id, terms);
    }
    ledger.record(operation_id, payer_guid, terms.kind(), verdict.err());
    Ok(())
}

/// Only the Guild Leader saves an emblem (`cm:GuildHandler.cpp:735-748`). Answers the Guild.
fn emblem_gate<L: FeeLedger>(ledger: &L, payer_guid: u64) -> Result<u32, GuildRefusal> {
    let (guild_id, leader_guid) = ledger
        .guild_of(payer_guid)
        .ok_or(GuildRefusal::NotInGuild)?;
    if leader_guid != payer_guid {
        return Err(GuildRefusal::NotLeader);
    }
    Ok(guild_id)
}

/// Phase 3. A Hold with another operation id, or none, means this finish already ran. Both
/// outcomes need the payer in the world here, so a Character in transit keeps its Hold for the
/// destination.
fn finish_fee<P: FeePurse>(
    purse: &mut P,
    operation_id: u64,
    payer_guid: u64,
    accepted: bool,
) -> Result<(), FeeError> {
    let Some(hold) = purse
        .held(payer_guid)
        .filter(|hold| hold.operation_id == operation_id)
    else {
        return Ok(());
    };
    if purse.purse(payer_guid).is_none() {
        return Err(FeeError::Conflict("payer is not in the world here"));
    }
    let refund = if accepted { 0 } else { hold.copper };
    purse.release(payer_guid, refund)
}

struct CtxPurse<'a> {
    ctx: &'a ReducerContext,
}

impl FeePurse for CtxPurse<'_> {
    fn held(&self, payer_guid: u64) -> Option<HeldFee> {
        self.ctx
            .db
            .game_guild_fee_hold()
            .payer_guid()
            .find(payer_guid)
            .and_then(HeldFee::from_row)
    }

    fn purse(&self, payer_guid: u64) -> Option<u32> {
        crate::helpers::acting_entity_by_guid(self.ctx, payer_guid).map(|payer| payer.money)
    }

    fn npc_serves(&self, payer_guid: u64, npc_guid: u64, kind: u8) -> bool {
        let flag = match kind {
            fee_kind::EMBLEM => npc_flags::TABARDDESIGNER,
            _ => return false,
        };
        let Ok((payer, npc)) = crate::items::npc_interaction_gate(
            self.ctx,
            payer_guid,
            npc_guid,
            flag,
            "tabard designer",
            "target is not a tabard designer",
            "design a tabard",
        ) else {
            return false;
        };
        !npc.dead
            && npc.health != 0
            && !crate::reputation::npc_refuses_interaction(self.ctx, &npc, &payer)
    }

    fn take(&mut self, hold: HeldFee) -> Result<(), FeeError> {
        let mut payer = crate::helpers::acting_entity_by_guid(self.ctx, hold.payer_guid)
            .ok_or(FeeError::Refused(GuildRefusal::NotEnoughMoney))?;
        payer.money = payer
            .money
            .checked_sub(hold.copper)
            .ok_or(FeeError::Refused(GuildRefusal::NotEnoughMoney))?;
        self.ctx.db.game_world_entity().guid().update(payer);
        let created_micros = self.ctx.timestamp.to_micros_since_unix_epoch();
        self.ctx
            .db
            .game_guild_fee_hold()
            .insert(hold.into_row(created_micros));
        Ok(())
    }

    fn release(&mut self, payer_guid: u64, refund: u32) -> Result<(), FeeError> {
        if refund != 0 {
            let mut payer = crate::helpers::acting_entity_by_guid(self.ctx, payer_guid)
                .ok_or(FeeError::Conflict("payer is not in the world here"))?;
            payer.money = crate::mail::credited(payer.money, refund);
            self.ctx.db.game_world_entity().guid().update(payer);
        }
        self.ctx
            .db
            .game_guild_fee_hold()
            .payer_guid()
            .delete(payer_guid);
        Ok(())
    }
}

struct CtxLedger<'a> {
    ctx: &'a ReducerContext,
}

impl FeeLedger for CtxLedger<'_> {
    fn decided_for(&self, operation_id: u64) -> Option<(u64, u8)> {
        self.ctx
            .db
            .game_guild_fee_decision()
            .operation_id()
            .find(operation_id)
            .map(|decision| (decision.payer_guid, decision.kind))
    }

    fn guild_of(&self, payer_guid: u64) -> Option<(u32, u64)> {
        let guild_id = member(self.ctx, payer_guid)?.guild_id;
        let guild = self.ctx.db.game_guild().guild_id().find(guild_id)?;
        Some((guild_id, guild.leader_guid))
    }

    fn apply(&mut self, guild_id: u32, terms: &GuildFeeTerms) {
        let GuildFeeTerms::Emblem(emblem) = terms;
        let Some(mut guild) = self.ctx.db.game_guild().guild_id().find(guild_id) else {
            return;
        };
        guild.emblem_style = emblem.emblem_style;
        guild.emblem_color = emblem.emblem_color;
        guild.border_style = emblem.border_style;
        guild.border_color = emblem.border_color;
        guild.background_color = emblem.background_color;
        self.ctx.db.game_guild().guild_id().update(guild);
        push_event(
            self.ctx,
            guild_id,
            0,
            event_kind::TABARD_CHANGED,
            0,
            0,
            Vec::new(),
        );
    }

    fn record(
        &mut self,
        operation_id: u64,
        payer_guid: u64,
        kind: u8,
        refusal: Option<GuildRefusal>,
    ) {
        self.ctx
            .db
            .game_guild_fee_decision()
            .insert(GuildFeeDecision {
                operation_id,
                payer_guid,
                kind,
                accepted: refusal.is_none(),
                refusal: refusal.map_or_else(String::new, |refusal| refusal.as_tag().to_string()),
                petition_id: 0,
                decided_micros: self.ctx.timestamp.to_micros_since_unix_epoch(),
            });
    }
}

/// Fee phase 1 on the payer's Home Shard: move the fee from the purse into a Fee Hold. The same
/// operation id with the same request is a replay and succeeds without a second debit.
#[reducer]
pub fn gw_guild_fee_hold(
    ctx: &ReducerContext,
    operation_id: u64,
    request_actor: crate::SessionActor,
    request: GuildFeeRequest,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let payer_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    hold_fee(
        &mut CtxPurse { ctx },
        HeldFee::requested(operation_id, payer_guid, &request),
    )
    .map_err(FeeError::into_text)
}

/// Fee phase 2 on Realm-core: commit the one decision for `operation_id`. Always Ok once a
/// decision exists for this payer and kind, accepted or refused.
#[reducer]
pub fn realm_guild_fee_decide(
    ctx: &ReducerContext,
    operation_id: u64,
    request_actor: crate::SessionActor,
    terms: GuildFeeTerms,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let payer_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    decide_fee(&mut CtxLedger { ctx }, operation_id, payer_guid, &terms)
        .map_err(FeeError::into_text)
}

/// Fee phase 3 on the payer's Home Shard: spend the Hold, or refund it when Realm-core refused,
/// then delete it. Without a Hold for `operation_id` it is a replay and changes nothing.
#[reducer]
pub fn gw_guild_fee_finish(
    ctx: &ReducerContext,
    operation_id: u64,
    request_actor: crate::SessionActor,
    accepted: bool,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let payer_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    finish_fee(&mut CtxPurse { ctx }, operation_id, payer_guid, accepted)
        .map_err(FeeError::into_text)
}

/// Character deletion must not destroy copper held for a guild operation.
pub(crate) fn character_has_fee_hold(ctx: &ReducerContext, character_guid: u64) -> bool {
    ctx.db
        .game_guild_fee_hold()
        .payer_guid()
        .find(character_guid)
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const LEADER: u64 = 5_090_401;
    const MEMBER: u64 = 5_090_402;
    const DESIGNER: u64 = 5_090_410;
    const GUILD: u32 = 7;

    const EMBLEM: GuildEmblem = GuildEmblem {
        emblem_style: 11,
        emblem_color: 12,
        border_style: 3,
        border_color: 14,
        background_color: 15,
    };

    fn purchase() -> GuildFeeRequest {
        GuildFeeRequest::Emblem(GuildEmblemPurchase {
            npc_guid: DESIGNER,
            emblem: EMBLEM,
        })
    }

    /// One Home Shard: purses of Characters in the world, the NPCs that serve them, the Holds.
    #[derive(Default)]
    struct Shard {
        purses: BTreeMap<u64, u32>,
        serving_npcs: Vec<u64>,
        holds: BTreeMap<u64, HeldFee>,
    }

    impl FeePurse for Shard {
        fn held(&self, payer_guid: u64) -> Option<HeldFee> {
            self.holds.get(&payer_guid).cloned()
        }

        fn purse(&self, payer_guid: u64) -> Option<u32> {
            self.purses.get(&payer_guid).copied()
        }

        fn npc_serves(&self, _payer_guid: u64, npc_guid: u64, kind: u8) -> bool {
            kind == fee_kind::EMBLEM && self.serving_npcs.contains(&npc_guid)
        }

        fn take(&mut self, hold: HeldFee) -> Result<(), FeeError> {
            let purse = self
                .purses
                .get_mut(&hold.payer_guid)
                .ok_or(FeeError::Refused(GuildRefusal::NotEnoughMoney))?;
            *purse = purse
                .checked_sub(hold.copper)
                .ok_or(FeeError::Refused(GuildRefusal::NotEnoughMoney))?;
            self.holds.insert(hold.payer_guid, hold);
            Ok(())
        }

        fn release(&mut self, payer_guid: u64, refund: u32) -> Result<(), FeeError> {
            let purse = self
                .purses
                .get_mut(&payer_guid)
                .ok_or(FeeError::Conflict("payer is not in the world here"))?;
            *purse = purse.saturating_add(refund);
            self.holds.remove(&payer_guid);
            Ok(())
        }
    }

    /// Realm-core: membership, leaders, emblems, decisions and TABARD_CHANGED events.
    #[derive(Default)]
    struct Realm {
        members: BTreeMap<u64, u32>,
        leaders: BTreeMap<u32, u64>,
        emblems: BTreeMap<u32, GuildEmblem>,
        decisions: BTreeMap<u64, (u64, u8, Option<GuildRefusal>)>,
        tabard_events: usize,
    }

    impl FeeLedger for Realm {
        fn decided_for(&self, operation_id: u64) -> Option<(u64, u8)> {
            self.decisions
                .get(&operation_id)
                .map(|(payer, kind, _)| (*payer, *kind))
        }

        fn guild_of(&self, payer_guid: u64) -> Option<(u32, u64)> {
            let guild_id = *self.members.get(&payer_guid)?;
            Some((guild_id, *self.leaders.get(&guild_id)?))
        }

        fn apply(&mut self, guild_id: u32, terms: &GuildFeeTerms) {
            let GuildFeeTerms::Emblem(emblem) = terms;
            self.emblems.insert(guild_id, *emblem);
            self.tabard_events += 1;
        }

        fn record(
            &mut self,
            operation_id: u64,
            payer_guid: u64,
            kind: u8,
            refusal: Option<GuildRefusal>,
        ) {
            self.decisions
                .insert(operation_id, (payer_guid, kind, refusal));
        }
    }

    fn shard(purse: u32) -> Shard {
        Shard {
            purses: BTreeMap::from([(LEADER, purse), (MEMBER, purse)]),
            serving_npcs: vec![DESIGNER],
            ..Shard::default()
        }
    }

    fn realm() -> Realm {
        Realm {
            members: BTreeMap::from([(LEADER, GUILD), (MEMBER, GUILD)]),
            leaders: BTreeMap::from([(GUILD, LEADER)]),
            ..Realm::default()
        }
    }

    fn hold(shard: &mut Shard, operation_id: u64, payer_guid: u64) -> Result<(), FeeError> {
        hold_fee(
            shard,
            HeldFee::requested(operation_id, payer_guid, &purchase()),
        )
    }

    fn decide(realm: &mut Realm, operation_id: u64, payer_guid: u64) -> Result<(), FeeError> {
        decide_fee(
            realm,
            operation_id,
            payer_guid,
            &GuildFeeTerms::Emblem(EMBLEM),
        )
    }

    fn accepted(realm: &Realm, operation_id: u64) -> bool {
        matches!(realm.decisions.get(&operation_id), Some((_, _, None)))
    }

    /// Hold, decide, finish with the decision the ledger recorded, as the Gateway drives it.
    fn pay(shard: &mut Shard, realm: &mut Realm, operation_id: u64, payer_guid: u64) {
        hold(shard, operation_id, payer_guid).unwrap();
        decide(realm, operation_id, payer_guid).unwrap();
        let accepted = accepted(realm, operation_id);
        finish_fee(shard, operation_id, payer_guid, accepted).unwrap();
    }

    #[test]
    fn the_guild_leader_buys_an_emblem_for_ten_gold() {
        let (mut shard, mut realm) = (shard(150_000), realm());
        pay(&mut shard, &mut realm, 1, LEADER);
        assert_eq!(shard.purses[&LEADER], 50_000);
        assert!(shard.holds.is_empty());
        assert_eq!(realm.emblems[&GUILD], EMBLEM);
        assert_eq!(realm.tabard_events, 1);
    }

    #[test]
    fn a_member_who_is_not_the_leader_is_refunded_every_copper() {
        let (mut shard, mut realm) = (shard(150_000), realm());
        pay(&mut shard, &mut realm, 1, MEMBER);
        assert_eq!(
            realm.decisions[&1],
            (MEMBER, fee_kind::EMBLEM, Some(GuildRefusal::NotLeader))
        );
        assert_eq!(shard.purses[&MEMBER], 150_000);
        assert!(shard.holds.is_empty());
        assert!(realm.emblems.is_empty());
    }

    #[test]
    fn leadership_that_moves_after_the_hold_refunds_the_old_leader() {
        let (mut shard, mut realm) = (shard(150_000), realm());
        hold(&mut shard, 1, LEADER).unwrap();
        realm.leaders.insert(GUILD, MEMBER);
        decide(&mut realm, 1, LEADER).unwrap();
        finish_fee(&mut shard, 1, LEADER, accepted(&realm, 1)).unwrap();
        assert_eq!(shard.purses[&LEADER], 150_000);
        assert!(realm.emblems.is_empty());
    }

    #[test]
    fn a_guild_that_disbands_after_the_hold_refunds_the_payer() {
        let (mut shard, mut realm) = (shard(150_000), realm());
        hold(&mut shard, 1, LEADER).unwrap();
        realm.members.clear();
        realm.leaders.clear();
        decide(&mut realm, 1, LEADER).unwrap();
        assert_eq!(
            realm.decisions[&1].2,
            Some(GuildRefusal::NotInGuild),
            "the vanished Guild is a refusal the finish can refund, not an error that strands the Hold"
        );
        finish_fee(&mut shard, 1, LEADER, accepted(&realm, 1)).unwrap();
        assert_eq!(shard.purses[&LEADER], 150_000);
        assert!(shard.holds.is_empty());
    }

    #[test]
    fn hold_refusals_change_nothing() {
        let mut broke = shard(99_999);
        assert_eq!(
            hold(&mut broke, 1, LEADER),
            Err(FeeError::Refused(GuildRefusal::NotEnoughMoney))
        );
        assert_eq!(broke.purses[&LEADER], 99_999);
        assert!(broke.holds.is_empty());

        let mut elsewhere = shard(150_000);
        elsewhere.serving_npcs.clear();
        assert_eq!(
            hold(&mut elsewhere, 1, LEADER),
            Err(FeeError::Refused(GuildRefusal::NpcRefused))
        );
        assert_eq!(elsewhere.purses[&LEADER], 150_000);
        assert!(elsewhere.holds.is_empty());

        let mut offline = shard(150_000);
        offline.purses.clear();
        assert_eq!(
            hold(&mut offline, 1, LEADER),
            Err(FeeError::Refused(GuildRefusal::NotEnoughMoney))
        );
        assert!(offline.holds.is_empty());
    }

    #[test]
    fn a_replayed_hold_takes_the_fee_once() {
        let mut shard = shard(250_000);
        hold(&mut shard, 1, LEADER).unwrap();
        hold(&mut shard, 1, LEADER).unwrap();
        assert_eq!(shard.purses[&LEADER], 150_000);
        assert_eq!(
            hold(&mut shard, 2, LEADER),
            Err(FeeError::Conflict("another Fee Hold is pending")),
            "a Character has one Fee Hold at a time"
        );
        assert_eq!(shard.purses[&LEADER], 150_000);
    }

    #[test]
    fn a_replayed_decision_applies_its_effect_once() {
        let mut realm = realm();
        decide(&mut realm, 1, LEADER).unwrap();
        realm.leaders.insert(GUILD, MEMBER);
        decide(&mut realm, 1, LEADER).unwrap();
        assert!(accepted(&realm, 1), "a retry reads the first decision back");
        assert_eq!(realm.tabard_events, 1);
        assert_eq!(
            decide(&mut realm, 1, MEMBER),
            Err(FeeError::Conflict("operation id reused"))
        );
    }

    #[test]
    fn a_replayed_finish_refunds_once() {
        let (mut shard, mut realm) = (shard(150_000), realm());
        hold(&mut shard, 1, MEMBER).unwrap();
        decide(&mut realm, 1, MEMBER).unwrap();
        finish_fee(&mut shard, 1, MEMBER, false).unwrap();
        finish_fee(&mut shard, 1, MEMBER, false).unwrap();
        assert_eq!(shard.purses[&MEMBER], 150_000);
    }

    #[test]
    fn a_stale_finish_leaves_a_newer_hold_alone() {
        let (mut shard, mut realm) = (shard(250_000), realm());
        pay(&mut shard, &mut realm, 1, MEMBER);
        hold(&mut shard, 2, MEMBER).unwrap();
        finish_fee(&mut shard, 1, MEMBER, false).unwrap();
        assert_eq!(shard.purses[&MEMBER], 150_000);
        assert_eq!(shard.holds[&MEMBER].operation_id, 2);
    }

    #[test]
    fn a_decision_made_before_the_hold_spends_the_fee_once() {
        let (mut shard, mut realm) = (shard(250_000), realm());
        decide(&mut realm, 1, LEADER).unwrap();
        finish_fee(&mut shard, 1, LEADER, true).unwrap();
        assert_eq!(
            shard.purses[&LEADER], 250_000,
            "a finish without its Hold spends nothing"
        );
        hold(&mut shard, 1, LEADER).unwrap();
        finish_fee(&mut shard, 1, LEADER, true).unwrap();
        finish_fee(&mut shard, 1, LEADER, true).unwrap();
        assert_eq!(shard.purses[&LEADER], 150_000);
        assert!(shard.holds.is_empty());
    }

    #[test]
    fn a_payer_out_of_the_world_keeps_the_hold_until_it_returns() {
        let (mut shard, mut realm) = (shard(150_000), realm());
        hold(&mut shard, 1, MEMBER).unwrap();
        decide(&mut realm, 1, MEMBER).unwrap();
        shard.purses.remove(&MEMBER);
        assert!(finish_fee(&mut shard, 1, MEMBER, false).is_err());
        assert_eq!(shard.holds[&MEMBER].copper, EMBLEM_COST_COPPER);
        shard.purses.insert(MEMBER, 50_000);
        finish_fee(&mut shard, 1, MEMBER, false).unwrap();
        assert_eq!(shard.purses[&MEMBER], 150_000);
        assert!(shard.holds.is_empty());
    }

    #[test]
    fn a_hold_row_round_trips_its_terms() {
        let hold = HeldFee::requested(9, LEADER, &purchase());
        assert_eq!(hold.copper, 100_000);
        assert_eq!(HeldFee::from_row(hold.clone().into_row(0)), Some(hold));
    }

    /// The actor guid is an argument, so the operator gate is the whole authorization.
    #[test]
    fn fee_reducers_open_with_the_operator_gate() {
        for signature in [
            "pub fn gw_guild_fee_hold(",
            "pub fn realm_guild_fee_decide(",
            "pub fn gw_guild_fee_finish(",
        ] {
            let body = crate::test_scan::code_of(include_str!("fee.rs"), signature);
            let normalized = body.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(
                normalized.starts_with("{ crate::helpers::require_operator(ctx)?;"),
                "`{signature}` no longer opens with the operator gate. Body was:\n{body}"
            );
        }
    }

    #[test]
    fn character_delete_refuses_a_fee_hold_before_the_cascade() {
        let body =
            crate::test_scan::code_of(include_str!("../auth.rs"), "pub fn delete_character(");
        let fee_gate = body
            .find("crate::guild::fee::character_has_fee_hold")
            .expect("character deletion must check for a Fee Hold");
        let cascade = body
            .find("crate::world::cascade_delete_character")
            .expect("character deletion still needs its normal cascade");
        assert!(
            fee_gate < cascade,
            "a Fee Hold must be fenced before deletion"
        );
    }
}
