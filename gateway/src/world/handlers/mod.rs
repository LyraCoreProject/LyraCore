//! Protocol Family handlers, their Stores, and shared protocol operations.

use super::*;

mod auction;
mod bank;
mod cast;
mod channel;
mod char;
mod chat;
mod combat;
mod death;
mod duel;
mod gameobject;
mod guild;
pub(crate) use gameobject::GameObject;
mod item;
mod loot;
mod mail;
mod meeting_stone;
mod melee;
mod member_stats;
mod query;
mod quest;
mod taxi;
mod trade;
mod trainer;
mod unavailable;
mod vendor;
mod weather;

#[cfg(test)]
pub(crate) use auction::tests::{store_with, InMemoryAuctionActions};
pub(crate) use auction::{
    decode_auction_browse, Auction, AuctionActionStore, AuctionBrowseRequest, AuctionPage,
    AuctionQuery, CancelAuctionOutcome, CancelAuctionRequest, CreateAuctionOutcome,
    CreateAuctionRequest, PlaceBidOutcome, PlaceBidRequest, CMSG_AUCTION_LIST_ITEMS_OPCODE,
};
pub(crate) use auction::{AuctionHousePolicy, AuctionInteraction};
pub(crate) use bank::{Bank, BankStore};
#[cfg(test)]
pub(crate) use cast::tests::InMemoryCasts;
pub(crate) use cast::{Cast, CastStore};
pub(crate) use channel::{resolve_online_character, ResolvedTarget};
pub(crate) use channel::{
    Channel, ChannelActionStore, ChannelOutcome, ChannelRequest, ChannelRoster,
};
pub(crate) use char::{Character, CharacterStore};
pub(crate) use chat::{
    Chat, ChatActionStore, ChatOutcome, RealmChatRequest, SpeakerFacts, SpeechStore,
    WhisperRequest, WhisperTargetFacts,
};
pub(crate) use combat::{Combat, CombatStore};
pub(crate) use death::DeathStore;
pub(crate) use duel::{Duel, DuelActionStore};
pub(crate) use guild::{
    character_facts, destroy_inert_charters, guild_projection, guild_sign_on, guild_world_entry,
    guild_world_exit, is_guild_dot_command, leads_a_guild, reconcile_deleted_guild_characters,
    run_guild_dot_command, CharacterFacts, DurableCharacterFacts, Guild, GuildActionStore,
    GuildCleanup, GuildEventSnapshot, GuildOutcome, GuildRequest,
};
#[cfg(test)]
pub(crate) use item::tests::InMemoryItemActions;
pub(crate) use item::{Item, ItemActionResult, ItemActionStore};
pub(crate) use loot::{
    Loot, LootActionStatus, LootWindow, LootWindowRefusal, LootWindowRequestStatus,
    LootWindowStore, OpenLootState,
};
pub(crate) use mail::Mail;
pub(crate) use meeting_stone::{
    MeetingStone, MeetingStoneActionStore, MeetingStoneOutcome, SeekerFacts,
};
#[cfg(test)]
pub(crate) use melee::tests::InMemoryMeleeActions;
pub(crate) use melee::{Melee, MeleeActionStore};
#[cfg(test)]
pub(crate) use member_stats::MemberSnapshot;
pub(crate) use member_stats::{
    member_stats_tick, MemberPresence, MemberStats, MemberStatsRecord, MemberStatsStore,
};
pub(crate) use query::{NpcStore, Query};
pub(crate) use quest::{quest_giver_menu, Quest, QuestActionStore};
#[cfg(test)]
pub(crate) use taxi::tests::InMemoryTaxiActions;
pub(crate) use taxi::{Taxi, TaxiActionStore};
pub(crate) use trade::{Trade, TradeStore};
pub(crate) use trainer::{Trainer, TrainerBuyOutcome, TrainerStore};
#[cfg(test)]
pub(crate) use vendor::tests::InMemoryVendorActions;
pub(crate) use vendor::{Vendor, VendorActionStore};
pub(crate) use weather::{zone_weather_message, WeatherStore};

/// Open the bank window for `banker_guid`. Single chokepoint for `CMSG_BANKER_ACTIVATE` and the
/// BANKER gossip option, so the two entry points cannot drift apart.
fn show_bank(banker_guid: u64) -> Outbound {
    Outbound::One(ServerOpcodeMessage::SMSG_SHOW_BANK(codec::build_show_bank(
        banker_guid,
    )))
}

pub(crate) use unavailable::{
    is_control_receipt, raw_unavailable_outbound, unavailable_outbound, UnavailableNotice,
};

pub(crate) use vendor::build_buyback_view_replay;
