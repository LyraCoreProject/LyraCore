//! Shared test support for World Session tests: [`WorldFake`], the multi-shard Fake every Store
//! family is implemented on, and its shard topology.

mod entry;
mod topology;
mod world_fake;

pub(crate) use self::entry::*;
pub(crate) use self::topology::*;
pub(crate) use self::world_fake::*;

use super::handlers::{
    resolve_online_character, AuctionActionStore, AuctionInteraction, CastStore,
    ChannelActionStore, ChannelOutcome, ChannelRequest, ChannelRoster, ChatActionStore,
    ChatOutcome, DuelActionStore, GuildActionStore, ItemActionStore, LootWindowRefusal,
    LootWindowRequestStatus, LootWindowStore, MeetingStoneActionStore, MeetingStoneOutcome,
    MeleeActionStore, MemberPresence, MemberSnapshot, MemberStatsStore, QuestActionStore,
    RealmChatRequest, ResolvedTarget, SpeakerFacts, TaxiActionStore, VendorActionStore,
    WeatherStore, WhisperRequest, WhisperTargetFacts,
};
use super::party::PartyOutcome;
use super::*;
use lyracore_shared::group::{GroupKind, GroupRefusal, RaidSlot};
use lyracore_shared::loot::LootRefusal;
use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;
