//! [`WorldFake`]: one sub-state per Store family, each in its family's file with its impl.

mod auction;
mod bank;
mod cast;
mod channel;
mod character;
mod chat;
mod combat;
mod death;
mod duel;
mod guild;
mod item;
mod loot_roll;
mod loot_window;
mod mail;
mod meeting_stone;
mod melee;
mod member_stats;
mod npc;
mod party;
mod quest;
mod session;
mod social;
mod speech;
mod taxi;
mod trade;
mod trainer;
mod transfer;
mod vendor;
mod weather;

pub(crate) use self::auction::*;
pub(crate) use self::cast::*;
pub(crate) use self::channel::*;
pub(crate) use self::character::*;
pub(crate) use self::chat::*;
pub(crate) use self::combat::*;
pub(crate) use self::guild::*;
pub(crate) use self::item::*;
pub(crate) use self::loot_roll::*;
pub(crate) use self::loot_window::*;
pub(crate) use self::mail::*;
pub(crate) use self::melee::*;
pub(crate) use self::member_stats::*;
pub(crate) use self::npc::*;
pub(crate) use self::party::*;
pub(crate) use self::quest::*;
pub(crate) use self::session::*;
pub(crate) use self::social::*;
pub(crate) use self::speech::*;
pub(crate) use self::taxi::*;
pub(crate) use self::trainer::*;
pub(crate) use self::transfer::*;
pub(crate) use self::vendor::*;
pub(crate) use self::weather::*;

use super::TopologyState;
use crate::world::codec;

/// The Fake behind World Session socket tests. It implements every Store family, so one handle
/// serves a whole session, and several handles wired through `topology` model a sharded Realm.
/// Each family keeps its state in its own field, defined beside that family's impl.
#[derive(Default)]
pub(crate) struct WorldFake {
    /// The Characters this Shard holds. Several families read it.
    pub(crate) characters: Vec<codec::CharacterView>,
    /// 195: `npc_refuses_interaction` return — false (derive-Default) keeps every fixture NPC open.
    pub(crate) npc_refuses: bool,
    /// When set, Durable Requests in several families fail with this error: a Refusal tag, or any
    /// other text for a failure with an unknown durable result.
    pub(crate) trade_error: Option<String>,
    pub(crate) topology: TopologyState,
    pub(crate) session: SessionState,
    pub(crate) character: CharacterState,
    pub(crate) transfer: TransferState,
    pub(crate) party: PartyState,
    pub(crate) mail: MailState,
    pub(crate) social: SocialState,
    pub(crate) npc: NpcState,
    pub(crate) trainer: TrainerState,
    pub(crate) combat: CombatState,
    pub(crate) cast: CastState,
    pub(crate) taxi: TaxiState,
    pub(crate) melee: MeleeState,
    pub(crate) chat: ChatState,
    pub(crate) speech: SpeechState,
    pub(crate) channel: ChannelState,
    pub(crate) guild: GuildState,
    pub(crate) auction: AuctionState,
    pub(crate) quest: QuestState,
    pub(crate) vendor: VendorState,
    pub(crate) item: ItemState,
    pub(crate) weather: WeatherState,
    pub(crate) member_stats: MemberStatsState,
    pub(crate) loot_window: LootWindowState,
    pub(crate) loot_roll: LootRollState,
}

pub(crate) fn lk<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.try_lock().expect(
        "re-entrant lock on FakeShardDb: a method is already holding this mutex further up the \
         stack. With `lock()` this would be a DEADLOCK and the suite would HANG instead of failing \
        — see the fn doc on `lk`.",
    )
}
