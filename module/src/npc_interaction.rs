use lyracore_shared::constants::{npc_flags, type_mask, unit_flags};
use spacetimedb::ReducerContext;

use crate::actor::{ActionRefusal, ActionRefusalKind};
use crate::{game_world_entity, WorldEntity};

// 10 yards, squared.
pub(crate) const INTERACTION_RANGE_SQ: f32 = 100.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NpcService {
    Vendor,
    Repair,
    Banker,
    Innkeeper,
    Trainer,
    Auctioneer,
    QuestGiver,
    Gossip,
    GuildEmblem,
    GuildCharter,
}

impl NpcService {
    fn served_by(self, flags: u32) -> bool {
        let required = match self {
            Self::Vendor => npc_flags::VENDOR,
            Self::Repair => npc_flags::REPAIR,
            Self::Banker => npc_flags::BANKER,
            Self::Innkeeper => npc_flags::INNKEEPER,
            Self::Trainer => npc_flags::TRAINER,
            Self::Auctioneer => npc_flags::AUCTIONEER,
            Self::QuestGiver => npc_flags::QUESTGIVER,
            Self::Gossip => return flags != 0,
            Self::GuildEmblem => npc_flags::TABARDDESIGNER,
            Self::GuildCharter => npc_flags::PETITIONER | npc_flags::TABARDDESIGNER,
        };
        flags & required == required
    }

    fn noun(self) -> &'static str {
        match self {
            Self::Vendor => "vendor",
            Self::Repair => "armorer",
            Self::Banker => "banker",
            Self::Innkeeper => "innkeeper",
            Self::Trainer => "trainer",
            Self::Auctioneer => "auctioneer",
            Self::QuestGiver => "quest giver",
            Self::Gossip => "gossip creature",
            Self::GuildEmblem | Self::GuildCharter => "guild NPC",
        }
    }

    fn verb(self) -> &'static str {
        match self {
            Self::Vendor => "trade with a vendor",
            Self::Repair => "repair",
            Self::Banker => "use the bank",
            Self::Innkeeper => "bind a home",
            Self::Trainer => "train",
            Self::Auctioneer => "use the auction house",
            Self::QuestGiver => "talk to a quest giver",
            Self::Gossip => "use gossip",
            Self::GuildEmblem | Self::GuildCharter => "pay a guild fee",
        }
    }

    fn wrong_service_text(self) -> &'static str {
        match self {
            Self::Vendor => "target is not a vendor",
            Self::Repair => "target cannot repair",
            Self::Banker => "target is not a banker",
            Self::Innkeeper => "target is not an innkeeper",
            Self::Trainer => "target is not a trainer",
            Self::Auctioneer => "target is not an auctioneer",
            Self::QuestGiver => "target is not a quest giver",
            Self::Gossip => "target has no gossip",
            Self::GuildEmblem | Self::GuildCharter => "target does not serve guild fees",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NpcGate {
    CharacterAbsent,
    CharacterDead,
    NpcAbsent,
    WrongService,
    NpcDead,
    NotSelectable,
    OtherMap,
    OutOfRange,
    Unfriendly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NpcRefusal {
    pub(crate) gate: NpcGate,
    pub(crate) service: NpcService,
}

// Innkeeper and trainer refusal text reaches the client as a system message.
impl std::fmt::Display for NpcRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let noun = self.service.noun();
        match (self.gate, self.service) {
            (NpcGate::CharacterAbsent, _) => f.write_str("user not in world"),
            (NpcGate::CharacterDead | NpcGate::NpcDead, NpcService::Trainer) => {
                f.write_str("training requires a living Character and trainer")
            }
            (NpcGate::CharacterDead, service) => {
                write!(f, "dead players cannot {}", service.verb())
            }
            (NpcGate::NpcAbsent, _) => write!(f, "no such {noun}"),
            (NpcGate::WrongService, service) => f.write_str(service.wrong_service_text()),
            (NpcGate::NpcDead, _) => write!(f, "{noun} is dead"),
            (NpcGate::NotSelectable, _) => write!(f, "{noun} cannot be selected"),
            (NpcGate::OtherMap, _) => write!(f, "{noun} on another map"),
            (NpcGate::OutOfRange, _) => write!(f, "{noun} out of range"),
            (NpcGate::Unfriendly, _) => write!(f, "{noun} refuses interaction"),
        }
    }
}

impl From<NpcRefusal> for String {
    fn from(refusal: NpcRefusal) -> Self {
        refusal.to_string()
    }
}

impl From<NpcRefusal> for crate::Refusal {
    fn from(refusal: NpcRefusal) -> Self {
        refusal.to_string().into()
    }
}

impl From<NpcRefusal> for ActionRefusal {
    fn from(refusal: NpcRefusal) -> Self {
        let kind = match refusal.gate {
            NpcGate::CharacterAbsent => ActionRefusalKind::MissingActor,
            NpcGate::CharacterDead => ActionRefusalKind::DeadActor,
            NpcGate::NpcAbsent => ActionRefusalKind::MissingTarget,
            NpcGate::NpcDead => ActionRefusalKind::DeadTarget,
            NpcGate::OtherMap => ActionRefusalKind::OtherPartition,
            NpcGate::OutOfRange => ActionRefusalKind::OutOfRange,
            NpcGate::NotSelectable | NpcGate::Unfriendly => ActionRefusalKind::CannotAct,
            NpcGate::WrongService => ActionRefusalKind::Other,
        };
        ActionRefusal::new(kind, refusal.to_string())
    }
}

/// The Actor and the NPC that offers it `service` (vmangos `Player::CanInteractWithNPC`).
pub(crate) fn gate(
    ctx: &ReducerContext,
    actor_guid: u64,
    npc_guid: u64,
    service: NpcService,
) -> Result<(WorldEntity, WorldEntity), NpcRefusal> {
    let actor = crate::helpers::acting_entity_by_guid(ctx, actor_guid)
        .filter(WorldEntity::is_player)
        .ok_or(NpcRefusal {
            gate: NpcGate::CharacterAbsent,
            service,
        })?;
    let npc = serving_npc(ctx, &actor, npc_guid, service)?;
    Ok((actor, npc))
}

pub(crate) fn serving_npc(
    ctx: &ReducerContext,
    actor: &WorldEntity,
    npc_guid: u64,
    service: NpcService,
) -> Result<WorldEntity, NpcRefusal> {
    let refuse = |gate| NpcRefusal { gate, service };
    if !alive(actor) {
        return Err(refuse(NpcGate::CharacterDead));
    }
    let npc = ctx
        .db
        .game_world_entity()
        .guid()
        .find(npc_guid)
        .ok_or(refuse(NpcGate::NpcAbsent))?;
    refusal(ctx, actor, &npc, service).map_or(Ok(npc), |gate| Err(refuse(gate)))
}

/// Finds an NPC within reach that offers `service`, for requests that name no NPC.
pub(crate) fn nearby(
    ctx: &ReducerContext,
    actor_guid: u64,
    service: NpcService,
) -> Result<WorldEntity, NpcRefusal> {
    let refuse = |gate| NpcRefusal { gate, service };
    let actor = crate::helpers::acting_entity_by_guid(ctx, actor_guid)
        .filter(WorldEntity::is_player)
        .ok_or(refuse(NpcGate::CharacterAbsent))?;
    if !alive(&actor) {
        return Err(refuse(NpcGate::CharacterDead));
    }
    crate::helpers::entities_near(
        ctx,
        actor.map_id,
        actor.instance_id,
        actor.x,
        actor.y,
        INTERACTION_RANGE_SQ.sqrt(),
    )
    .into_iter()
    .find(|npc| refusal(ctx, &actor, npc, service).is_none())
    .ok_or(refuse(NpcGate::OutOfRange))
}

fn alive(entity: &WorldEntity) -> bool {
    !entity.dead && entity.health != 0
}

fn refusal(
    ctx: &ReducerContext,
    actor: &WorldEntity,
    npc: &WorldEntity,
    service: NpcService,
) -> Option<NpcGate> {
    if npc.is_player()
        || npc.type_mask & type_mask::CREATURE != type_mask::CREATURE
        || !service.served_by(npc.npc_flags)
    {
        return Some(NpcGate::WrongService);
    }
    if !alive(npc) {
        return Some(NpcGate::NpcDead);
    }
    if npc.unit_flags & unit_flags::NOT_SELECTABLE != 0 {
        return Some(NpcGate::NotSelectable);
    }
    if !crate::helpers::in_same_partition(npc, actor.map_id, actor.instance_id) {
        return Some(NpcGate::OtherMap);
    }
    if crate::helpers::dist_sq(actor, npc) > INTERACTION_RANGE_SQ {
        return Some(NpcGate::OutOfRange);
    }
    if crate::reputation::npc_refuses_interaction(ctx, npc, actor) {
        return Some(NpcGate::Unfriendly);
    }
    None
}
