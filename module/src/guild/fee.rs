//! The Fee Hold: how a guild operation that costs copper pays across two databases.
//!
//! The purse lives on the payer's Home Shard. Whether the operation is allowed is known only on
//! Realm-core. No transaction spans both, so one fee takes three Durable Requests, each idempotent
//! on the Gateway's operation id:
//!
//! 1. [`gw_guild_fee_hold`] on the Home Shard runs the NPC and purse Gates and moves the fee from
//!    the purse into the payer's Fee Hold. A Guild Charter is created in the same step.
//! 2. [`realm_guild_fee_decide`] on Realm-core runs the guild Gates and commits one decision row
//!    for the operation id, with the accepted effect in the same transaction. A refusal is data in
//!    that row, so a retry gets the same answer.
//! 3. [`gw_guild_fee_finish`] on the Home Shard spends the Hold on acceptance or puts the copper
//!    back on refusal, and deletes the Hold last. A refused Guild Charter is destroyed and refunded
//!    only while the payer still holds it, so a Charter the payer destroyed is never paid back.
//!
//! What the fee pays for can vanish between the phases: the Guild disbands, leadership moves. The
//! decision then refuses and the finish refunds, so copper never strands. A Hold that a crash left
//! behind travels with its Character on Transfer, and the Gateway finishes it at the next world
//! entry. Character deletion is refused while one exists.
//!
//! The Gateway mints a fresh operation id inside each hold call and never replays that call, so a
//! Hold is taken at most once per operation id; a replay of an id that Realm-core already decided
//! would take the fee a second time. It always holds before it decides. A finish or decide that
//! arrives without its Hold spends nothing.

use lyracore_shared::constants::npc_flags;
use lyracore_shared::guild::{
    event_kind, fee_kind, name_key, validate_guild_name, GuildRefusal, CHARTER_COST_COPPER,
    EMBLEM_COST_COPPER, GUILD_CHARTER_ENTRY,
};
use spacetimedb::{reducer, table, ReducerContext, SpacetimeType, Table};

use super::{game_guild, member, petition, push_event};
use crate::game_world_entity;
use crate::items::{game_item_instance, game_item_template};

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
    /// The Tabard Designer or Petitioner the payer stood at.
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

/// A Guild Charter bought at the Petitioner `npc_guid`, proposing a Guild named `name`.
#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct GuildCharterPurchase {
    pub npc_guid: u64,
    pub name: String,
}

/// What the payer asks its Home Shard to hold copper for. Later fee kinds append variants.
#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub enum GuildFeeRequest {
    Emblem(GuildEmblemPurchase),
    Charter(GuildCharterPurchase),
}

/// A Guild Charter purchase as Realm-core decides it: the Charter the Hold created and its name,
/// read back from the Hold, plus the owner's name and team, which the Gateway conveys because
/// Realm-core holds no Character rows.
#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct GuildCharterTerms {
    pub charter_item_guid: u64,
    pub name: String,
    pub payer_name: String,
    pub payer_team: u32,
}

/// What Realm-core decides on: the operation a Fee Hold pays for, read back from the Hold.
#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub enum GuildFeeTerms {
    Emblem(GuildEmblem),
    Charter(GuildCharterTerms),
}

impl GuildFeeTerms {
    fn kind(&self) -> u8 {
        match self {
            Self::Emblem(_) => fee_kind::EMBLEM,
            Self::Charter(_) => fee_kind::CHARTER,
        }
    }
}

/// What a Fee Hold pays for, as the Home Shard stores it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum HeldTerms {
    Emblem(GuildEmblem),
    /// `charter_item_guid` is 0 until the hold creates the Charter.
    Charter {
        name: String,
        charter_item_guid: u64,
    },
}

impl HeldTerms {
    fn kind(&self) -> u8 {
        match self {
            Self::Emblem(_) => fee_kind::EMBLEM,
            Self::Charter { .. } => fee_kind::CHARTER,
        }
    }

    fn copper(&self) -> u32 {
        match self {
            Self::Emblem(_) => EMBLEM_COST_COPPER,
            Self::Charter { .. } => CHARTER_COST_COPPER,
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
    terms: HeldTerms,
}

impl HeldFee {
    fn requested(operation_id: u64, payer_guid: u64, request: &GuildFeeRequest) -> Self {
        let (npc_guid, terms) = match request {
            GuildFeeRequest::Emblem(purchase) => {
                (purchase.npc_guid, HeldTerms::Emblem(purchase.emblem))
            }
            GuildFeeRequest::Charter(purchase) => (
                purchase.npc_guid,
                HeldTerms::Charter {
                    name: purchase.name.clone(),
                    charter_item_guid: 0,
                },
            ),
        };
        Self {
            operation_id,
            payer_guid,
            copper: terms.copper(),
            npc_guid,
            terms,
        }
    }

    /// Is this Hold what `request` asked for? The hold mints the Charter guid, so a replay of the
    /// same request matches whatever guid the first hold minted.
    fn answers(&self, request: &HeldFee) -> bool {
        let mut minted = request.clone();
        if let (
            HeldTerms::Charter {
                charter_item_guid, ..
            },
            HeldTerms::Charter {
                charter_item_guid: held,
                ..
            },
        ) = (&mut minted.terms, &self.terms)
        {
            *charter_item_guid = *held;
        }
        *self == minted
    }

    fn from_row(row: GuildFeeHold) -> Option<Self> {
        let terms = match row.kind {
            fee_kind::EMBLEM => HeldTerms::Emblem(GuildEmblem {
                emblem_style: row.emblem_style,
                emblem_color: row.emblem_color,
                border_style: row.border_style,
                border_color: row.border_color,
                background_color: row.background_color,
            }),
            fee_kind::CHARTER => HeldTerms::Charter {
                name: row.charter_name,
                charter_item_guid: row.charter_item_guid,
            },
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
        let (emblem, charter_name, charter_item_guid) = match self.terms {
            HeldTerms::Emblem(emblem) => (emblem, String::new(), 0),
            HeldTerms::Charter {
                name,
                charter_item_guid,
            } => (GuildEmblem::default(), name, charter_item_guid),
        };
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
            charter_item_guid,
            charter_name,
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

/// The Home Shard half: the payer's purse, its Guild Charters and its Fee Hold.
trait FeePurse {
    fn held(&self, payer_guid: u64) -> Option<HeldFee>;
    /// The payer's purse, `None` when the payer is not in the world on this Shard.
    fn purse(&self, payer_guid: u64) -> Option<u32>;
    /// Does `npc_guid` serve fee `kind` to the payer here: flag, alive, in reach, not refusing?
    fn npc_serves(&self, payer_guid: u64, npc_guid: u64, kind: u8) -> bool;
    /// Does this Shard know the Guild Charter item?
    fn charter_known(&self) -> bool;
    /// Can the payer carry one more Guild Charter? The item's unique count comes before a free
    /// bag slot, as `Player::CanStoreNewItem` checks them (`cm:PetitionsHandler.cpp:128-134`).
    fn charter_room(&self, payer_guid: u64) -> Result<(), GuildRefusal>;
    /// Take `hold.copper` from the purse and insert the Hold. A Charter Hold also creates the
    /// Charter and records its guid.
    fn take(&mut self, hold: HeldFee) -> Result<(), FeeError>;
    /// Destroy the Guild Charter `charter_item_guid` if the payer still holds it. Answers whether
    /// it did.
    fn take_back_charter(&mut self, payer_guid: u64, charter_item_guid: u64) -> bool;
    /// Put `refund` back in the purse and delete the Hold.
    fn release(&mut self, payer_guid: u64, refund: u32) -> Result<(), FeeError>;
}

/// The Realm-core half: guild facts, Petitions and the decision ledger.
trait FeeLedger {
    /// The payer and kind a decision for `operation_id` was made for.
    fn decided_for(&self, operation_id: u64) -> Option<(u64, u8)>;
    /// The payer's Guild and its Guild Leader, when the payer is a member.
    fn guild_of(&self, payer_guid: u64) -> Option<(u32, u64)>;
    fn owns_petition(&self, payer_guid: u64) -> bool;
    fn guild_name_taken(&self, name: &str) -> bool;
    fn apply_emblem(&mut self, guild_id: u32, emblem: &GuildEmblem);
    /// Open the Petition of an accepted Charter. Answers its id.
    fn open_petition(&mut self, payer_guid: u64, terms: &GuildCharterTerms) -> u32;
    fn record(
        &mut self,
        operation_id: u64,
        payer_guid: u64,
        kind: u8,
        refusal: Option<GuildRefusal>,
        petition_id: u32,
    );
}

/// Phase 1. Gates in mangos order: the NPC, the Charter template, the purse, then a bag slot for
/// the Charter (`cm:GuildHandler.cpp:726-755`, `cm:PetitionsHandler.cpp:76-134`).
fn hold_fee<P: FeePurse>(purse: &mut P, hold: HeldFee) -> Result<(), FeeError> {
    if hold.operation_id == 0 {
        return Err(FeeError::Conflict("operation id 0"));
    }
    if let Some(existing) = purse.held(hold.payer_guid) {
        return if existing.answers(&hold) {
            Ok(())
        } else {
            Err(FeeError::Conflict("another Fee Hold is pending"))
        };
    }
    let charter = matches!(hold.terms, HeldTerms::Charter { .. });
    if !purse.npc_serves(hold.payer_guid, hold.npc_guid, hold.terms.kind()) {
        return Err(FeeError::Refused(GuildRefusal::NpcRefused));
    }
    if charter && !purse.charter_known() {
        return Err(FeeError::Refused(GuildRefusal::CharterUnavailable));
    }
    if purse
        .purse(hold.payer_guid)
        .is_none_or(|copper| copper < hold.copper)
    {
        return Err(FeeError::Refused(GuildRefusal::NotEnoughMoney));
    }
    if charter {
        purse
            .charter_room(hold.payer_guid)
            .map_err(FeeError::Refused)?;
    }
    purse.take(hold)
}

/// Does one more Guild Charter fit under the item's unique count? `max_count` 0 means no limit.
fn charter_fits(max_count: u32, held: usize) -> bool {
    max_count == 0 || held < max_count as usize
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
        GuildFeeTerms::Emblem(emblem) => emblem_gate(ledger, payer_guid).map(|guild_id| {
            ledger.apply_emblem(guild_id, emblem);
            0
        }),
        GuildFeeTerms::Charter(charter) => {
            if charter.charter_item_guid == 0 {
                return Err(FeeError::Conflict("Guild Charter guid 0"));
            }
            charter_gate(ledger, payer_guid, charter)
                .map(|()| ledger.open_petition(payer_guid, charter))
        }
    };
    let (refusal, petition_id) = match verdict {
        Ok(petition_id) => (None, petition_id),
        Err(refusal) => (Some(refusal), 0),
    };
    ledger.record(operation_id, payer_guid, terms.kind(), refusal, petition_id);
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

/// A Guild Charter opens a Petition for a Character outside every Guild that owns none, under a
/// valid name no Guild holds (`cm:PetitionsHandler.cpp:88-112`,
/// `vm:src/game/Handlers/PetitionsHandler.cpp:69-71`).
fn charter_gate<L: FeeLedger>(
    ledger: &L,
    payer_guid: u64,
    terms: &GuildCharterTerms,
) -> Result<(), GuildRefusal> {
    if ledger.guild_of(payer_guid).is_some() {
        return Err(GuildRefusal::AlreadyInGuild);
    }
    if ledger.owns_petition(payer_guid) {
        return Err(GuildRefusal::AlreadyHasPetition);
    }
    validate_guild_name(&terms.name)?;
    if ledger.guild_name_taken(&terms.name) {
        return Err(GuildRefusal::NameExists);
    }
    Ok(())
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
    let refund = match &hold.terms {
        _ if accepted => 0,
        HeldTerms::Emblem(_) => hold.copper,
        HeldTerms::Charter {
            charter_item_guid, ..
        } => {
            if purse.take_back_charter(payer_guid, *charter_item_guid) {
                hold.copper
            } else {
                0
            }
        }
    };
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
        // A Guild Charter needs a Petitioner that is also a Tabard Designer
        // (`cm:PetitionsHandler.cpp:76-84`). The interaction Gate accepts any one of the flags, so
        // every one of them is checked after it.
        let required_flags = match kind {
            fee_kind::EMBLEM => npc_flags::TABARDDESIGNER,
            fee_kind::CHARTER => npc_flags::PETITIONER | npc_flags::TABARDDESIGNER,
            _ => return false,
        };
        let Ok((payer, npc)) = crate::items::npc_interaction_gate(
            self.ctx,
            payer_guid,
            npc_guid,
            required_flags,
            "guild NPC",
            "target does not serve guild fees",
            "pay a guild fee",
        ) else {
            return false;
        };
        npc.npc_flags & required_flags == required_flags
            && !npc.dead
            && npc.health != 0
            && !crate::reputation::npc_refuses_interaction(self.ctx, &npc, &payer)
    }

    fn charter_known(&self) -> bool {
        self.ctx
            .db
            .game_item_template()
            .entry()
            .find(GUILD_CHARTER_ENTRY)
            .is_some()
    }

    fn charter_room(&self, payer_guid: u64) -> Result<(), GuildRefusal> {
        let max_count = self
            .ctx
            .db
            .game_item_template()
            .entry()
            .find(GUILD_CHARTER_ENTRY)
            .map_or(0, |template| template.max_count);
        let held = self
            .ctx
            .db
            .game_item_instance()
            .by_owner_guid()
            .filter(&payer_guid)
            .filter(|item| item.entry == GUILD_CHARTER_ENTRY)
            .count();
        if !charter_fits(max_count, held) {
            return Err(GuildRefusal::CharterLimit);
        }
        if !crate::items::has_free_slot(self.ctx, payer_guid) {
            return Err(GuildRefusal::BagsFull);
        }
        Ok(())
    }

    fn take(&mut self, mut hold: HeldFee) -> Result<(), FeeError> {
        let mut payer = crate::helpers::acting_entity_by_guid(self.ctx, hold.payer_guid)
            .ok_or(FeeError::Refused(GuildRefusal::NotEnoughMoney))?;
        payer.money = payer
            .money
            .checked_sub(hold.copper)
            .ok_or(FeeError::Refused(GuildRefusal::NotEnoughMoney))?;
        let owner_identity = payer.owner_identity;
        self.ctx.db.game_world_entity().guid().update(payer);
        if let HeldTerms::Charter {
            charter_item_guid, ..
        } = &mut hold.terms
        {
            *charter_item_guid = self.create_charter(hold.payer_guid, owner_identity)?;
        }
        let created_micros = self.ctx.timestamp.to_micros_since_unix_epoch();
        self.ctx
            .db
            .game_guild_fee_hold()
            .insert(hold.into_row(created_micros));
        Ok(())
    }

    fn take_back_charter(&mut self, payer_guid: u64, charter_item_guid: u64) -> bool {
        let items = self.ctx.db.game_item_instance();
        let held = items
            .guid()
            .find(charter_item_guid)
            .is_some_and(|item| item.owner_guid == payer_guid && item.entry == GUILD_CHARTER_ENTRY);
        if held {
            items.guid().delete(charter_item_guid);
        }
        held
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

impl CtxPurse<'_> {
    /// Mint one Guild Charter into the payer's bags. Answers its guid.
    fn create_charter(
        &self,
        payer_guid: u64,
        owner_identity: spacetimedb::Identity,
    ) -> Result<u64, FeeError> {
        let template = self
            .ctx
            .db
            .game_item_template()
            .entry()
            .find(GUILD_CHARTER_ENTRY)
            .ok_or(FeeError::Refused(GuildRefusal::CharterUnavailable))?;
        let guid = crate::items::next_item_guid(self.ctx)
            .map_err(|_| FeeError::Conflict("no item guid left"))?;
        crate::items::store_instance_state(
            self.ctx,
            payer_guid,
            owner_identity,
            &template,
            Some(guid),
            &crate::items::ItemSnapshot {
                entry: GUILD_CHARTER_ENTRY,
                stack_count: 1,
                durability: template.max_durability,
                ..crate::items::ItemSnapshot::default()
            },
        )
        .map_err(|_| FeeError::Refused(GuildRefusal::BagsFull))?;
        Ok(guid)
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

    fn owns_petition(&self, payer_guid: u64) -> bool {
        petition::owns_petition(self.ctx, payer_guid)
    }

    fn guild_name_taken(&self, name: &str) -> bool {
        self.ctx
            .db
            .game_guild()
            .name_key()
            .find(name_key(name))
            .is_some()
    }

    fn apply_emblem(&mut self, guild_id: u32, emblem: &GuildEmblem) {
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

    fn open_petition(&mut self, payer_guid: u64, terms: &GuildCharterTerms) -> u32 {
        petition::open(
            self.ctx,
            payer_guid,
            &terms.payer_name,
            terms.payer_team,
            &terms.name,
            terms.charter_item_guid,
        )
    }

    fn record(
        &mut self,
        operation_id: u64,
        payer_guid: u64,
        kind: u8,
        refusal: Option<GuildRefusal>,
        petition_id: u32,
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
                petition_id,
                decided_micros: self.ctx.timestamp.to_micros_since_unix_epoch(),
            });
    }
}

/// Fee phase 1 on the payer's Home Shard: move the fee from the purse into a Fee Hold. While that
/// Hold exists, the same operation id with the same request is a replay and succeeds without a
/// second debit. After the finish deletes the Hold, a replay would take the fee again, so the
/// Gateway never replays a hold.
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

    /// One Home Shard: purses of Characters in the world, the NPCs that serve them, the Guild
    /// Charters each Character holds, and the Holds.
    #[derive(Default)]
    struct Shard {
        purses: BTreeMap<u64, u32>,
        serving_npcs: Vec<u64>,
        holds: BTreeMap<u64, HeldFee>,
        /// Guild Charter guid to its holder.
        charters: BTreeMap<u64, u64>,
        charter_unknown: bool,
        /// What the payer's bags answer one more Guild Charter.
        charter_room: Option<GuildRefusal>,
    }

    impl FeePurse for Shard {
        fn held(&self, payer_guid: u64) -> Option<HeldFee> {
            self.holds.get(&payer_guid).cloned()
        }

        fn purse(&self, payer_guid: u64) -> Option<u32> {
            self.purses.get(&payer_guid).copied()
        }

        fn npc_serves(&self, _payer_guid: u64, npc_guid: u64, _kind: u8) -> bool {
            self.serving_npcs.contains(&npc_guid)
        }

        fn charter_known(&self) -> bool {
            !self.charter_unknown
        }

        fn charter_room(&self, _payer_guid: u64) -> Result<(), GuildRefusal> {
            self.charter_room.map_or(Ok(()), Err)
        }

        fn take(&mut self, mut hold: HeldFee) -> Result<(), FeeError> {
            let purse = self
                .purses
                .get_mut(&hold.payer_guid)
                .ok_or(FeeError::Refused(GuildRefusal::NotEnoughMoney))?;
            *purse = purse
                .checked_sub(hold.copper)
                .ok_or(FeeError::Refused(GuildRefusal::NotEnoughMoney))?;
            if let HeldTerms::Charter {
                charter_item_guid, ..
            } = &mut hold.terms
            {
                *charter_item_guid = CHARTER_GUID_BASE + self.charters.len() as u64;
                self.charters.insert(*charter_item_guid, hold.payer_guid);
            }
            self.holds.insert(hold.payer_guid, hold);
            Ok(())
        }

        fn take_back_charter(&mut self, payer_guid: u64, charter_item_guid: u64) -> bool {
            if self.charters.get(&charter_item_guid) == Some(&payer_guid) {
                self.charters.remove(&charter_item_guid);
                return true;
            }
            false
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

    /// Realm-core: membership, leaders, emblems, Petitions, decisions and TABARD_CHANGED events.
    #[derive(Default)]
    struct Realm {
        members: BTreeMap<u64, u32>,
        leaders: BTreeMap<u32, u64>,
        guild_names: Vec<String>,
        emblems: BTreeMap<u32, GuildEmblem>,
        /// Owner to its Petition id and the terms that opened it.
        petitions: BTreeMap<u64, (u32, GuildCharterTerms)>,
        decisions: BTreeMap<u64, (u64, u8, Option<GuildRefusal>)>,
        decided_petitions: BTreeMap<u64, u32>,
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

        fn owns_petition(&self, payer_guid: u64) -> bool {
            self.petitions.contains_key(&payer_guid)
        }

        fn guild_name_taken(&self, name: &str) -> bool {
            self.guild_names
                .iter()
                .any(|taken| name_key(taken) == name_key(name))
        }

        fn apply_emblem(&mut self, guild_id: u32, emblem: &GuildEmblem) {
            self.emblems.insert(guild_id, *emblem);
            self.tabard_events += 1;
        }

        fn open_petition(&mut self, payer_guid: u64, terms: &GuildCharterTerms) -> u32 {
            let petition_id = 40 + self.petitions.len() as u32;
            self.petitions
                .insert(payer_guid, (petition_id, terms.clone()));
            petition_id
        }

        fn record(
            &mut self,
            operation_id: u64,
            payer_guid: u64,
            kind: u8,
            refusal: Option<GuildRefusal>,
            petition_id: u32,
        ) {
            self.decisions
                .insert(operation_id, (payer_guid, kind, refusal));
            self.decided_petitions.insert(operation_id, petition_id);
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

    const PETITIONER: u64 = 5_090_411;
    const OUTSIDER: u64 = 5_090_403;
    const CHARTER_GUID_BASE: u64 = 5_090_420;

    fn charter_purchase(name: &str) -> GuildFeeRequest {
        GuildFeeRequest::Charter(GuildCharterPurchase {
            npc_guid: PETITIONER,
            name: name.into(),
        })
    }

    fn guild_master_shard(purse: u32) -> Shard {
        Shard {
            purses: BTreeMap::from([(OUTSIDER, purse), (MEMBER, purse)]),
            serving_npcs: vec![PETITIONER],
            ..Shard::default()
        }
    }

    fn hold_charter(shard: &mut Shard, operation_id: u64, name: &str) -> Result<(), FeeError> {
        hold_fee(
            shard,
            HeldFee::requested(operation_id, OUTSIDER, &charter_purchase(name)),
        )
    }

    /// Decide the Charter the shard holds for `payer_guid`, as the Gateway conveys it from the
    /// Hold plus the payer's name and team.
    fn decide_charter(
        shard: &Shard,
        realm: &mut Realm,
        operation_id: u64,
        payer_guid: u64,
    ) -> Result<(), FeeError> {
        let HeldTerms::Charter {
            name,
            charter_item_guid,
        } = shard.holds[&payer_guid].terms.clone()
        else {
            panic!("not a Charter Hold");
        };
        decide_fee(
            realm,
            operation_id,
            payer_guid,
            &GuildFeeTerms::Charter(GuildCharterTerms {
                charter_item_guid,
                name,
                payer_name: "Founder".into(),
                payer_team: 469,
            }),
        )
    }

    #[test]
    fn a_guild_charter_costs_ten_silver_and_opens_a_petition() {
        let (mut shard, mut realm) = (guild_master_shard(1_500), realm());
        hold_charter(&mut shard, 1, "Night Watch").unwrap();
        assert_eq!(shard.purses[&OUTSIDER], 500);
        assert_eq!(shard.charters, BTreeMap::from([(5_090_420, OUTSIDER)]));
        decide_charter(&shard, &mut realm, 1, OUTSIDER).unwrap();
        finish_fee(&mut shard, 1, OUTSIDER, accepted(&realm, 1)).unwrap();

        assert_eq!(shard.purses[&OUTSIDER], 500);
        assert!(shard.holds.is_empty());
        assert_eq!(shard.charters, BTreeMap::from([(5_090_420, OUTSIDER)]));
        assert_eq!(
            realm.petitions[&OUTSIDER],
            (
                40,
                GuildCharterTerms {
                    charter_item_guid: 5_090_420,
                    name: "Night Watch".into(),
                    payer_name: "Founder".into(),
                    payer_team: 469,
                }
            )
        );
        assert_eq!(realm.decided_petitions[&1], 40);
    }

    #[test]
    fn charter_hold_gates_run_in_mangos_order_and_change_nothing() {
        let cases = [
            (
                Shard {
                    serving_npcs: Vec::new(),
                    charter_unknown: true,
                    ..guild_master_shard(999)
                },
                GuildRefusal::NpcRefused,
            ),
            (
                Shard {
                    charter_unknown: true,
                    charter_room: Some(GuildRefusal::BagsFull),
                    ..guild_master_shard(999)
                },
                GuildRefusal::CharterUnavailable,
            ),
            (
                Shard {
                    charter_room: Some(GuildRefusal::CharterLimit),
                    ..guild_master_shard(999)
                },
                GuildRefusal::NotEnoughMoney,
            ),
            (
                Shard {
                    charter_room: Some(GuildRefusal::CharterLimit),
                    ..guild_master_shard(1_000)
                },
                GuildRefusal::CharterLimit,
            ),
            (
                Shard {
                    charter_room: Some(GuildRefusal::BagsFull),
                    ..guild_master_shard(1_000)
                },
                GuildRefusal::BagsFull,
            ),
        ];
        for (mut shard, refusal) in cases {
            let purse = shard.purses[&OUTSIDER];
            assert_eq!(
                hold_charter(&mut shard, 1, "Night Watch"),
                Err(FeeError::Refused(refusal))
            );
            assert_eq!(shard.purses[&OUTSIDER], purse, "{refusal:?}");
            assert!(shard.holds.is_empty() && shard.charters.is_empty());
        }
    }

    /// Item 5863 is unique (`max_count` 1): a Character who kept its first Charter through a
    /// Guild it has since left holds one and cannot carry a second.
    #[test]
    fn a_unique_charter_fits_only_under_its_max_count() {
        assert!(charter_fits(1, 0));
        assert!(!charter_fits(1, 1));
        assert!(charter_fits(0, 7), "max_count 0 sets no limit");
    }

    #[test]
    fn a_replayed_charter_hold_creates_one_charter() {
        let mut shard = guild_master_shard(2_500);
        hold_charter(&mut shard, 1, "Night Watch").unwrap();
        hold_charter(&mut shard, 1, "Night Watch").unwrap();
        assert_eq!(shard.purses[&OUTSIDER], 1_500);
        assert_eq!(shard.charters.len(), 1);
        assert_eq!(
            hold_charter(&mut shard, 1, "Day Watch"),
            Err(FeeError::Conflict("another Fee Hold is pending"))
        );
    }

    #[test]
    fn charter_decision_gates_run_in_order() {
        let cases = [
            (MEMBER, "Night Watch", GuildRefusal::AlreadyInGuild),
            (OUTSIDER, "Night!Watch", GuildRefusal::AlreadyHasPetition),
            (5_090_404, "Night!Watch", GuildRefusal::NameInvalid),
            (5_090_404, "KNIGHTS", GuildRefusal::NameExists),
        ];
        for (payer, name, refusal) in cases {
            let mut realm = Realm {
                guild_names: vec!["Knights".into()],
                ..realm()
            };
            realm.petitions.insert(
                OUTSIDER,
                (
                    39,
                    GuildCharterTerms {
                        charter_item_guid: 5_090_429,
                        name: "Old".into(),
                        payer_name: "Founder".into(),
                        payer_team: 469,
                    },
                ),
            );
            let terms = GuildFeeTerms::Charter(GuildCharterTerms {
                charter_item_guid: 5_090_420,
                name: name.into(),
                payer_name: "Payer".into(),
                payer_team: 469,
            });
            decide_fee(&mut realm, 1, payer, &terms).unwrap();
            assert_eq!(
                realm.decisions[&1],
                (payer, fee_kind::CHARTER, Some(refusal)),
                "{name}"
            );
            assert_eq!(realm.petitions.len(), 1, "a refusal opens no Petition");
        }
    }

    #[test]
    fn a_refused_charter_is_destroyed_and_refunded() {
        let (mut shard, mut realm) = (guild_master_shard(1_500), realm());
        realm.guild_names.push("Night Watch".into());
        hold_charter(&mut shard, 1, "Night Watch").unwrap();
        decide_charter(&shard, &mut realm, 1, OUTSIDER).unwrap();
        finish_fee(&mut shard, 1, OUTSIDER, accepted(&realm, 1)).unwrap();
        assert_eq!(shard.purses[&OUTSIDER], 1_500);
        assert!(shard.charters.is_empty() && shard.holds.is_empty());
        assert!(realm.petitions.is_empty());
    }

    #[test]
    fn a_charter_that_left_the_payer_before_a_refusal_is_not_refunded() {
        let (mut shard, mut realm) = (guild_master_shard(1_500), realm());
        realm.guild_names.push("Night Watch".into());
        hold_charter(&mut shard, 1, "Night Watch").unwrap();
        decide_charter(&shard, &mut realm, 1, OUTSIDER).unwrap();
        shard.charters.insert(5_090_420, MEMBER);
        finish_fee(&mut shard, 1, OUTSIDER, accepted(&realm, 1)).unwrap();
        assert_eq!(
            shard.purses[&OUTSIDER], 500,
            "the Charter's new holder keeps it, so the copper stays spent"
        );
        assert_eq!(shard.charters, BTreeMap::from([(5_090_420, MEMBER)]));
        assert!(shard.holds.is_empty());
    }

    #[test]
    fn a_charter_decision_without_its_charter_guid_decides_nothing() {
        let mut realm = realm();
        let terms = GuildFeeTerms::Charter(GuildCharterTerms {
            charter_item_guid: 0,
            name: "Night Watch".into(),
            payer_name: "Founder".into(),
            payer_team: 469,
        });
        assert_eq!(
            decide_fee(&mut realm, 1, OUTSIDER, &terms),
            Err(FeeError::Conflict("Guild Charter guid 0"))
        );
        assert!(realm.decisions.is_empty());
    }

    #[test]
    fn a_charter_hold_row_round_trips_its_terms() {
        let mut hold = HeldFee::requested(9, OUTSIDER, &charter_purchase("Night Watch"));
        assert_eq!(hold.copper, 1_000);
        if let HeldTerms::Charter {
            charter_item_guid, ..
        } = &mut hold.terms
        {
            *charter_item_guid = 5_090_420;
        }
        let row = hold.clone().into_row(0);
        assert_eq!(
            (row.kind, row.charter_item_guid, row.charter_name.as_str()),
            (2, 5_090_420, "Night Watch")
        );
        assert_eq!(HeldFee::from_row(row), Some(hold));
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
