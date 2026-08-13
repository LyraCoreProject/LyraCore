use super::handlers::{ItemActionStore, VendorActionStore};
use super::*;
use std::os::unix::net::UnixStream;

/// The client side of every real world-session test has a bounded read. A missing server packet is
/// a test failure, never an indefinitely blocked test process.
const WORLD_SESSION_READ_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

fn world_session_socket_pair() -> (UnixStream, UnixStream) {
    let (client, server) = UnixStream::pair().expect("world-session socket pair must be created");
    client
        .set_read_timeout(Some(WORLD_SESSION_READ_DEADLINE))
        .expect("world-session client read deadline must be configured");
    (client, server)
}

#[test]
fn world_session_socket_pair_times_out_when_the_server_writes_nothing() {
    let (mut client, _server) = world_session_socket_pair();
    let mut byte = [0];
    let error = client
        .read(&mut byte)
        .expect_err("a silent server must hit the world-session read deadline");
    assert!(
        matches!(
            error.kind(),
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        ),
        "a silent server must time out, got {error}"
    );
}

/// The realm-wide party routing tests. A child module so they can reach
/// `InMemoryStore` and its fake realm-core topology without widening anything, kept in their own
/// file because this one is already the largest in the tree.
#[path = "party_tests.rs"]
mod party_tests;

/// The realm-wide whisper routing tests. A sibling of `party_tests` for the
/// same reason — it reaches `InMemoryStore` (and `party_tests`' live topology) without widening
/// anything.
#[path = "whisper_tests.rs"]
mod whisper_tests;

/// The realm-wide loot-roll routing/relay tests. A sibling of `party_tests`/`whisper_tests` for
/// the same reason — it reaches `InMemoryStore` without widening anything.
#[path = "loot_tests.rs"]
mod loot_tests;

/// The mailbox read-path routing tests. A sibling of the modules above for the same reason — it
/// reaches `InMemoryStore` (and `party_tests`' fixture characters) without widening anything.
#[path = "mail_tests.rs"]
mod mail_tests;

/// The inbound FRAMING boundary — malformed, truncated, oversized and unsupported packets
/// driven as raw bytes over a real cipher. A sibling of the modules above for the same reason (it
/// reaches `InMemoryStore` and `client_handshake`), kept separate because it is the only file here
/// that writes headers no typed builder can produce.
#[path = "framing_tests.rs"]
mod framing_tests;

/// The per-account connection release regressions. A sibling of the modules above for the
/// same reason — it reaches `InMemoryStore` without widening anything.

/// Multi-shard routing — reducer calls and subscriptions never target a shard other than the
/// player's home shard. A sibling of the modules above for the same reason. `ShardCallLog` is
/// `pub(super)` here and re-exported below because this file's own pre-shard-routing tests (and
/// `transfer_tests`) still share the one call-log type.
#[path = "shard_routing_tests.rs"]
mod shard_routing_tests;
use shard_routing_tests::ShardCallLog;

/// Cross-database transfer — Phase A of the elastic world-sharding design (escrowed transfers,
/// instance/continent shards, region-level load balancing). A sibling of the
/// modules above for the same reason; its `FakeShardDb`/`FakeChar`/`lk` fixtures stay in THIS file
/// (see the comment above `struct FakeChar` below) because this file's own `Store` impl and two
/// earlier regression tests construct them directly.
#[path = "transfer_tests.rs"]
mod transfer_tests;

#[path = "trade_tests.rs"]
mod trade_tests;
/// `SMSG_COMPRESSED_MOVES` corruption regressions, driven through the real `spawn_writer` +
/// `wow_srp` cipher pair. A sibling of the modules above for the same reason.
#[path = "wire_corruption_tests.rs"]
mod wire_corruption_tests;

use wow_world_base::shared::friend_result_vanilla_tbc::FriendResult;
use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;
use wow_world_messages::vanilla::{
    BuyBankSlotResult,
    BuyResult,
    BuybackSlot,
    Class,
    ClientMessage,
    Gender,
    GroupLootSetting,
    ItemQuality,
    ItemSlot,
    Language,
    Level,
    Map,
    Object,
    Race,
    RollVote,
    SheathState,
    SpellCastTargets,
    SpellCastTargets_SpellCastTargetFlags,
    SpellCastTargets_SpellCastTargetFlags_Item,
    SpellCastTargets_SpellCastTargetFlags_Unit,
    Talent,
    TrainingFailureReason,
    WorldResult,
    CMSG_ADD_FRIEND,
    CMSG_ADD_IGNORE,
    CMSG_ATTACKSTOP,
    CMSG_ATTACKSWING,
    CMSG_AUTH_SESSION,
    CMSG_AUTOBANK_ITEM,
    CMSG_AUTOEQUIP_ITEM,
    // Inventory + death/resurrection dispatch tests.
    CMSG_AUTOSTORE_BAG_ITEM,
    CMSG_AUTOSTORE_BANK_ITEM,
    CMSG_BANKER_ACTIVATE,
    CMSG_BUYBACK_ITEM,
    CMSG_BUY_BANK_SLOT,
    CMSG_BUY_ITEM,
    CMSG_CANCEL_AURA,
    CMSG_CANCEL_CAST,
    CMSG_CAST_SPELL,
    CMSG_CHAR_CREATE,
    CMSG_CHAR_DELETE,
    CMSG_CHAR_ENUM,
    CMSG_DEL_FRIEND,
    CMSG_DEL_IGNORE,
    CMSG_FRIEND_LIST,
    CMSG_GOSSIP_HELLO,
    CMSG_GOSSIP_SELECT_OPTION,
    CMSG_INSPECT,
    CMSG_LEARN_TALENT,
    CMSG_LIST_INVENTORY,
    CMSG_LOGOUT_REQUEST,
    CMSG_LOOT,
    CMSG_LOOT_MASTER_GIVE,
    CMSG_LOOT_METHOD,
    CMSG_LOOT_MONEY,
    CMSG_LOOT_RELEASE,
    CMSG_LOOT_ROLL,
    CMSG_NPC_TEXT_QUERY,
    CMSG_PLAYED_TIME,
    CMSG_PLAYER_LOGIN,
    CMSG_PUSHQUESTTOPARTY,
    CMSG_QUESTGIVER_ACCEPT_QUEST,
    CMSG_QUESTGIVER_CHOOSE_REWARD,
    CMSG_QUESTGIVER_COMPLETE_QUEST,
    CMSG_QUESTGIVER_HELLO,
    CMSG_QUESTGIVER_STATUS_QUERY,
    CMSG_QUESTLOG_REMOVE_QUEST,
    // Item guid → slot resolution (vendor sell / armorer repair).
    CMSG_RECLAIM_CORPSE,
    CMSG_REPAIR_ITEM,
    CMSG_REPOP_REQUEST,
    CMSG_RESURRECT_RESPONSE,
    CMSG_SELL_ITEM,
    CMSG_SETSHEATHED,
    CMSG_SET_SELECTION,
    CMSG_SPIRIT_HEALER_ACTIVATE,
    CMSG_SWAP_INV_ITEM,
    CMSG_SWAP_ITEM,
    CMSG_TRAINER_BUY_SPELL,
    CMSG_TRAINER_LIST,
    // Item-starts-quest + party sharing.
    CMSG_USE_ITEM,
    CMSG_WHO,
    // Cross-map teleport: the client's world-port-finished ack.
    MSG_MOVE_WORLDPORT_ACK,
};
use wow_world_messages::Guid;

const K: [u8; 40] = [
    0x2E, 0xFE, 0xE7, 0xB0, 0xC1, 0x77, 0xEB, 0xBD, 0xFF, 0x66, 0x76, 0xC5, 0x6E, 0xFC, 0x23, 0x39,
    0xBE, 0x9C, 0xAD, 0x14, 0xBF, 0x8B, 0x54, 0xBB, 0x5A, 0x86, 0xFB, 0xF8, 0x1F, 0x6D, 0x42, 0x4A,
    0xA2, 0x3C, 0xC9, 0xA3, 0x14, 0x9F, 0xB1, 0x75,
];

/// In-memory `WorldStore` returning a fixed session key + character list for one account.
/// One recorded `movement_update`: (opcode, x, y, z, orientation, timestamp).
type MoveRecord = (u32, f32, f32, f32, f32, u32);

#[derive(Default)]
struct InMemoryStore {
    /// WORLDPORT_ACK gate: true = entity present -> a spurious ack is ignored;
    /// false (derive-Default) = absent -> a genuine transfer is pending.
    entity_in_world: bool,
    /// An in-session controllable cache answer for movement desync regression tests.
    entity_presence: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    entity_presence_checks: std::sync::atomic::AtomicUsize,
    username: String,
    session: Option<WorldSession>,
    characters: Vec<codec::CharacterView>,
    login_entity: Option<codec::EntityView>,
    moves: std::sync::Mutex<Vec<MoveRecord>>,
    /// Vendor stock returned by `vendor_items` (empty by default).
    vendor_stock: Vec<codec::VendorItemView>,
    /// 195: `npc_refuses_interaction` return — false (derive-Default) keeps every fixture NPC open.
    npc_refuses: bool,
    /// Spelled as a refusal so derive-Default (false) keeps every fixture trainer serving; the
    /// trait method reads the negation.
    trainer_refuses_class: bool,
    /// When set, buy/sell return this error (a gameplay failure) instead of `Ok`.
    trade_error: Option<String>,
    /// Quest-giver evals returned by `quest_giver_evals` (the menu/status input).
    quest_evals: Vec<codec::GiverQuestEval>,
    /// Quest details `quest_detail(id)` resolves from (matched by `quest_id`).
    quest_details: Vec<codec::QuestDetailView>,
    /// The player's quest-log slots `player_quest_log` returns (drives abandon's slot→id resolution).
    quest_log_slots: Vec<codec::update_mask::QuestLogSlot>,
    /// Recorded quest reducer dispatches: (account, giver, quest) for accept/turn-in; (account, quest)
    /// for abandon — so E2E tests assert the RIGHT reducer ran with the RIGHT args.
    accepted: std::sync::Mutex<Vec<(u64, u64, u32)>>,
    turned_in: std::sync::Mutex<Vec<(u64, u64, u32, u32)>>,
    abandoned: std::sync::Mutex<Vec<(u64, u32)>>,
    /// Override for `player_combat_until_ms`: 0 = out of combat (default), non-zero = in combat until
    /// this ms-epoch deadline (use u64::MAX for "always in combat" in tests).
    combat_until_ms: u64,
    /// Tracks whether `logout` was called (entity removal path taken).
    logout_called: std::sync::atomic::AtomicBool,
    /// When set, teardown cannot reach the database after a session-fatal transport loss.
    /// The world session must still close and relinquish its admission seat.
    logout_error: Option<String>,
    /// Recorded `delete_character` calls: (account_id, character_guid).
    deleted: std::sync::Mutex<Vec<(u64, u64)>>,
    /// When set, `delete_character` returns this outcome instead of `Success`.
    delete_outcome: Option<codec::CharDeleteOutcome>,
    /// Characters actually produced by a `create_character` call during the test, unioned
    /// into `characters()`'s answer. Without this, a CMSG_CHAR_CREATE round trip is a no-op the
    /// fake immediately forgets, so a test driving CREATE then CMSG_CHAR_ENUM/CMSG_PLAYER_LOGIN
    /// for "the character just created" would actually be exercising a hardcoded/pre-seeded guid
    /// with no real link to the CREATE call — the tautology a review caught.
    created_characters: std::sync::Mutex<Vec<codec::CharacterView>>,
    /// The Nth guid `create_character` assigns, offset well above every hand-seeded fixture
    /// guid in this file (the highest is 100, in the transfer tests) so it can never collide.
    next_created_guid: std::sync::atomic::AtomicU64,
    /// Reputation standings `player_reputations` returns — `(reputation_index, standing)` pairs folded
    /// into the login SMSG_INITIALIZE_FACTIONS (restoring persisted standings instead of the
    /// all-neutral stub).
    reputations: Vec<(i32, i32, bool)>,
    /// Imported action-bar rows `player_actions` returns — `(button, action, action_type)` triples.
    /// Empty by default (the pre-import fallback path).
    player_actions: Vec<(u8, u32, u8)>,
    /// Friend/ignore rows: `(owner_guid, target_guid, is_ignore)`. `add_friend`/
    /// `add_ignore`/`del_friend`/`del_ignore` mutate it; `contact_lists` reads it scoped to the caller.
    contacts: std::sync::Mutex<Vec<(u64, u64, bool)>>,
    group_invites: std::sync::Mutex<Vec<u64>>,
    /// When set, `start_attack` returns this error (drives the ATTACKSWING dead/friendly/desync split).
    start_attack_error: Option<String>,
    /// When set, `start_ranged_attack` returns this error (Auto Shot failure → SMSG_CAST_RESULT).
    start_ranged_attack_error: Option<String>,
    /// When set, `cast_spell` returns this error (cast rejection → SMSG_CAST_RESULT Failure).
    cast_spell_error: Option<String>,
    /// When set, `send_whisper` returns this error (→ SMSG_CHAT_PLAYER_NOT_FOUND).
    whisper_error: Option<String>,
    /// Recorded `send_whisper` calls — `(target_player, message)`, the TYPED
    /// NAME as the pre-realm-core path passes it (the module resolves it). The single-database plane's
    /// byte-identity is asserted against this.
    whispers: std::sync::Mutex<Vec<(String, String)>>,
    /// When set, `party_chat` returns this error — e.g. `group_err::NOT_IN_GROUP`
    /// to drive the "not in a group" → `SMSG_PARTY_COMMAND_RESULT(NotInGroup)` mapping.
    party_chat_error: Option<String>,
    /// Recorded `party_chat` messages — the dispatch test asserts the RIGHT text
    /// reached the reducer call.
    party_chats: std::sync::Mutex<Vec<String>>,
    /// When set, `gm_command` returns this error — e.g. `"permission denied"` to
    /// drive the Say-handler's `Err` → self-only `SMSG_MESSAGECHAT` System relay.
    gm_command_error: Option<String>,
    /// Recorded `gm_command` dispatches — the dot-command divert test asserts the
    /// RIGHT raw text (still carrying its leading `.`) reached the reducer call, and that a NON-dot
    /// Say never reaches this vec at all.
    gm_commands: std::sync::Mutex<Vec<String>>,
    /// Recorded `cast_spell` dispatches: (spell_id, target_guid) — pins target threading.
    casts: std::sync::Mutex<Vec<(u32, u64)>>,
    // Test recorder: the tuple is the recorded CALL's argument list, so it tracks the verb it records.
    #[allow(clippy::type_complexity)]
    /// Ground-targeted casts routed via `cast_spell_at`: (spell_id, target_guid, x, y, z).
    ground_casts: std::sync::Mutex<Vec<(u32, u64, f32, f32, f32)>>,
    /// Recorded `start_ranged_attack` dispatches: (target_guid, spell_id) — the Auto Shot intercept.
    ranged_attacks: std::sync::Mutex<Vec<(u64, u32)>>,
    /// Recorded `set_sheathed` dispatches: (self_guid, state) — the `CMSG_SETSHEATHED` route (#101).
    sheathed: std::sync::Mutex<Vec<(u64, u8)>>,
    /// What `spell_cast_time` returns: None (default) = unknown spell (the handler treats it as
    /// instant), Some(t) = the game_spell header's cast_time_ms.
    cast_time_ms: Option<u32>,
    queues_next_swing: bool,
    channel_joins: std::sync::Mutex<Vec<String>>,
    channel_messages: std::sync::Mutex<Vec<(String, String)>>,
    /// Enchant/disenchant routing `enchant_route` returns (None = a normal cast).
    enchant_route: Option<super::EnchantRoute>,
    /// Item-guid → bag-slot fixture backing `item_slot_by_guid`.
    item_slots: Vec<(u64, u8)>,
    /// Recorded `enchant_item_on_slot` calls: (slot, enchant_id).
    enchanted: std::sync::Mutex<Vec<(u8, u32)>>,
    /// Recorded `disenchant_item` slots.
    disenchanted: std::sync::Mutex<Vec<u8>>,
    /// The lootable copper `loot_target_money` reports for any target (default 0).
    corpse_money: u32,
    /// Recorded `loot_money` targets — CMSG_LOOT_MONEY must drive the TRACKED guid.
    money_looted: std::sync::Mutex<Vec<u64>>,
    /// Recorded `skin_corpse` targets (the empty-loot-window skinning fallback).
    skinned: std::sync::Mutex<Vec<u64>>,
    /// Recorded `buyback_item` calls: (vendor_guid, slot) — pins the 69→0 slot mapping.
    bought_back: std::sync::Mutex<Vec<(u64, u8)>>,
    /// What `talent_grant_spell` returns (0 = passive talent → no SMSG_LEARNED_SPELL push).
    talent_grant: u32,
    /// What `talent_pane_sync` returns: (teach rank-spell, superseded prev, points remaining).
    talent_pane: (u32, u32, u32),
    /// Spell ids `spell_is_fishing` claims.
    fishing_spells: Vec<u32>,
    /// Count of `fish` reducer dispatches.
    fish_casts: std::sync::atomic::AtomicU64,
    /// Spell ids `spell_is_open_lock` claims (Pick Lock).
    open_lock_spells: Vec<u32>,
    /// Recorded `pick_lock` reducer dispatches: the target GO guid decoded off the cast.
    pick_lock_casts: std::sync::Mutex<Vec<u64>>,
    /// `npc_is_innkeeper` flag for the gossip bind-home routing.
    innkeeper: bool,
    /// Whether `bind_home` ran (the innkeeper gossip select).
    home_bound: std::sync::atomic::AtomicBool,
    /// Recorded `reset_talents` dispatches: (account_id, self_guid, trainer_guid) — the unlearn-talents
    /// gossip select (#516).
    reset_talents_calls: std::sync::Mutex<Vec<(u64, u64, u64)>>,
    /// When set, `reset_talents` returns this error instead of recording the call.
    reset_talents_error: Option<String>,
    /// Recorded `send_chat` lines: (chat_type, language, message).
    chats: std::sync::Mutex<Vec<(u8, u8, String)>>,
    /// When true, `release_session` reports the epoch superseded (stale socket) — the world-side
    /// half of the session-epoch arbitration: `leave_world` must then SKIP the `logout` reducer.
    stale_session: bool,
    /// Imported gossip menu options `gossip_options` returns for ANY npc_guid — empty
    /// by default (the pre-import fallback path).
    gossip_opts: Vec<codec::GossipOptionView>,
    /// Recorded `gossip_select` notifications as `(option_id, option_row_id)` — the clicked POSITION
    /// and the stable row identity the module is told about.
    gossip_selects: std::sync::Mutex<Vec<(u32, u32)>>,
    /// The caller's quest log for `quest_status`, as `(quest_id, rewarded)` pairs — a quest id present
    /// here is "taken"; `rewarded` distinguishes active vs. turned-in. Absent = never seen.
    /// Behind a `Mutex` so a test can change the log WHILE a gossip window is open — the
    /// HELLO→SELECT race the menu snapshot exists to close.
    quest_log: std::sync::Mutex<Vec<(u32, bool)>>,
    /// The `npc_text_for_id` view `npc_text_for_id` returns for ANY text_id — `None` by default (the
    /// generic-greeting fallback), settable per-test for the 8-slot pin coverage.
    npc_text_view: Option<codec::NpcTextView>,
    /// Per-VIEWER corpse loot fixture for `corpse_loot(corpse_guid, viewer_guid)` — different viewers
    /// of the SAME corpse can see different windows (`quest_only` rows are per-looter) — keyed by
    /// viewer guid, standing in for whatever the real per-viewer read
    /// (`gateway/src/stdb/reads.rs::corpse_loot`) would return for that viewer; its own filtering
    /// decision is unit-tested directly in `reads.rs`, not reproduced here. Empty by default — every
    /// test that never sets this keeps seeing an empty window, byte-identical to before.
    corpse_loot_by_viewer: std::collections::HashMap<u64, Vec<codec::LootItemView>>,
    /// Recorded `group_loot_method` calls: (loot_setting, master_guid, loot_threshold).
    group_loot_methods: std::sync::Mutex<Vec<(u8, u64, u8)>>,
    /// Recorded `loot_roll` calls: (corpse_guid, loot_slot, vote).
    loot_rolls: std::sync::Mutex<Vec<(u64, u32, u8)>>,
    /// Recorded `loot_master_give` calls: (corpse_guid, loot_slot, target_guid).
    loot_master_gives: std::sync::Mutex<Vec<(u64, u8, u64)>>,
    /// `item_start_quest` fixture (item-starts-quest) — `Some((item_guid, quest_id))`
    /// makes CMSG_USE_ITEM open the quest details screen instead of consuming the item; `None`
    /// (default) is the pre-item-starts-quest behavior (every item goes through the normal
    /// `use_item` consume path).
    item_start_quest_fixture: Option<(u64, u32)>,
    /// Recorded `use_item` slots — the non-consumption test proves this stays EMPTY when
    /// `item_start_quest_fixture` intercepts the use.
    used_items: std::sync::Mutex<Vec<u8>>,
    /// Recorded `push_quest` calls: (account_id, quest_id) — quest sharing.
    pushed_quests: std::sync::Mutex<Vec<(u64, u32)>>,
    /// When set, `push_quest` returns this error instead of `Ok`.
    push_quest_error: Option<String>,
    /// Recorded `player_login` call count — the WORLDPORT_ACK test distinguishes the
    /// initial `CMSG_PLAYER_LOGIN` call from a world-port RE-entry call, since both dispatch through
    /// this one trait method (`enter_world` is shared by both call sites).
    login_calls: std::sync::atomic::AtomicU32,
    /// When set, every `player_login` call AFTER the first returns THIS entity instead of
    /// `login_entity` — simulates the character row having moved to a new map (`teleport_player`'s
    /// durable write) between the initial login and the `MSG_MOVE_WORLDPORT_ACK`. `None` (default):
    /// every call keeps returning `login_entity`, byte-identical to before this field existed.
    worldport_entity: Option<codec::EntityView>,
    /// When set, every `player_login` AFTER the first FAILS with this message — the world entry
    /// behind a world-port that routed fine (the stranding guard, a refused re-login on the
    /// destination shard). The client is mid-loading-screen for it either way.
    worldport_login_error: Option<String>,
    /// Recorded `subscribe_player_events` calls: (self_guid, login_map, login_x, login_y) — the
    /// WORLDPORT_ACK test asserts this fires AGAIN (a fresh `created` dedup set) at the new
    /// map/position rather than reusing the old subscription.
    subscribed: std::sync::Mutex<Vec<(u64, u32, f32, f32)>>,
    /// The egress DEPTH counter of the live session `subscribe_player_events` was handed — so a test
    /// can read the real queue depth of a real `run_world_session` (the writer thread's decrement has
    /// no other reachable seam: it lives inside the spawned writer loop). Deliberately the depth
    /// `Arc` and NOT the `SessionTx` itself: holding a sender clone here would keep the writer's
    /// `rx.recv()` alive forever and hang every `enter_world` test's `server.join()`.
    session_depth: std::sync::Mutex<Option<std::sync::Arc<std::sync::atomic::AtomicUsize>>>,
    /// Multi-shard routing: the database this handle stands for. `""` (derive-Default) is the
    /// single-shard world every other test runs in, where nothing routes.
    shard: String,
    /// The handle `home_shard()` hands back — the character's home shard. `None` (default) = "you
    /// are already on the right shard", i.e. the single-entry shard map / pre-sharding behavior.
    home: Option<std::sync::Arc<InMemoryStore>>,
    /// Home-shard reassignment: when set, every `home_shard()` resolution AFTER the first
    /// answers THIS shard instead of `home` — the mock's stand-in for a routing change landing
    /// between two logins (a shard-map edit, or the realm-core index re-homing a character). `None`
    /// (default): every resolution answers `home`, byte-identical to before this field existed.
    home_after_flip: Option<std::sync::Arc<InMemoryStore>>,
    /// How many times `home_shard()` has been asked — drives `home_after_flip`, and is itself the
    /// assertion that routing is resolved ONCE PER WORLD ENTRY and never mid-session. SHARED
    /// between a store and the handles it routes to (like `calls`), so a re-resolution asked of the
    /// *pinned* handle — which is what a mid-session re-route would actually look like, since
    /// `route_home` asks whichever handle the session currently holds — is counted too.
    home_shard_calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    /// SHARED between a store and its `home` handle: `(shard, call)` for every instrumented
    /// player-scoped call, in order. The routing test asserts nothing lands on the wrong database.
    calls: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
    /// The fake DATABASE this handle talks to, for the cross-database transfer tests. `None`
    /// (the default) leaves every transfer trait method at its "this store does not shard" default,
    /// so every other test in this file is untouched.
    xdb: Option<std::sync::Arc<FakeShardDb>>,
    /// The handle `realm_store()` hands back — the database that owns party
    /// membership realm-wide. `None` (derive-Default) is the SINGLE-DATABASE gateway, which is what
    /// every other test in this file is, and it is what routes every party op back onto the
    /// player-facing reducers below.
    realm: Option<std::sync::Arc<InMemoryStore>>,
    /// The connected WORLD shards `world_stores()` fans the roster mirror out to (and the
    /// cross-shard name/presence lookups walk). Empty = single database. Behind a `Mutex` only so
    /// the topology can be wired up AFTER every handle exists — production reads the shared
    /// `ShardSet`, which has the same shape and the same "includes this handle" membership.
    peers: std::sync::Mutex<Vec<std::sync::Arc<InMemoryStore>>>,
    /// The AUTHORITATIVE party state, when this handle is the realm-core one. Shared with
    /// nobody — a realm handle owns exactly one of these, and every shard reads its own `mirror`.
    party: std::sync::Arc<std::sync::Mutex<FakeParty>>,
    /// True when this handle is realm-core, so `group_roster` answers from `party` (the
    /// authority) instead of `mirror` (this shard's cache of it).
    is_realm: bool,
    /// What `sync_group_mirror` wrote onto THIS shard, latest per group. The invalidation
    /// story, made observable.
    mirror: std::sync::Mutex<Vec<super::party::GroupRoster>>,
    /// Guids with a LIVE entity on this shard — the per-guid `entity_in_world` answer a
    /// realm-wide party frame's online flags are built from. Empty = the single `entity_in_world`
    /// flag above decides, as it did before.
    live_guids: Vec<u64>,
    /// Seeded characters that are nevertheless OFFLINE, so the invite gate's "player not
    /// online" arm can be driven. Empty = every seeded character is online, as before.
    offline_guids: Vec<u64>,
    /// When set, `sync_group_mirror` fails with this message — a world shard that cannot be
    /// mirrored (an unreachable database), which must not fail a party op realm-core already took.
    mirror_error: Option<String>,
    /// What `realm_whisper` was asked to deliver on THIS handle —
    /// `(sender_guid, target_guid, message, sender_is_ignored)`. The realm handle owns the list; a
    /// world shard's staying empty is how a test tells "the whisper went to the authority" from "it
    /// quietly went back to being shard-local".
    realm_whispers: std::sync::Mutex<Vec<(u64, u64, String, bool)>>,
    /// When set, `realm_whisper` fails with this message — an unreachable
    /// realm-core, which must still leave the player with the same refusal packet they always got.
    realm_whisper_error: Option<String>,
    /// When set, `contact_lists` fails with this message on THIS shard — the
    /// unreachable-database arm of the realm-wide ignore-list union.
    contact_lists_error: Option<String>,
    /// The mail rows on THIS database, as `(recipient_guid, row)`. The realm handle owns them on a
    /// sharded gateway and a world shard's staying empty is how a test tells "the mailbox read went
    /// to the authority" from "it quietly went back to being shard-local"; on a single-database
    /// gateway the one handle owns them instead. Same fixture either way — that is the point.
    mails: std::sync::Mutex<Vec<(u64, codec::MailView)>>,
    /// Gameobject guids that ARE a mailbox within reach on this shard. Empty (derive-Default)
    /// refuses every mailbox, which is the wrong-map / out-of-range / not-a-mailbox arm.
    mailboxes: Vec<u64>,
    /// `game_world_entity.money` per guid, on THIS database — the purse the postage comes out of.
    /// A guid with no row here cannot pay, which is also the module's answer for a character with no
    /// live entity on the shard being asked.
    purses: std::sync::Mutex<Vec<(u64, u32)>>,
    /// Letters written on THIS database: `(sender, recipient, subject, body, attached money)`.
    /// Recorded separately from `mails` because WHICH call wrote the row is what tells the two
    /// planes apart — `mail_send` on one database, `mail_commit` on the mail plane of a sharded one.
    #[allow(clippy::type_complexity)]
    sent_mail: std::sync::Mutex<Vec<(u64, u64, String, String, u32)>>,
    /// The escrow ledger on THIS database, as `(the guid the fence is filed under, the row)`. A
    /// letter's fence is filed under its SENDER and lives on their shard; a take's is filed under
    /// the PAYEE and lives on the plane holding the mail row.
    #[allow(clippy::type_complexity)]
    mail_escrows: std::sync::Mutex<Vec<(u64, mail::HeldEscrow)>>,
    /// `game_mail_escrow.delivered` per escrow id: the attestation that licenses the settle. Kept
    /// beside the fence rather than in it so the fake cannot settle one it never attested.
    attested: std::sync::Mutex<Vec<(u64, bool)>>,
    /// Delivery/payout receipts on THIS database, `(escrow_id, recipient or payee)` — the
    /// idempotency key that makes a replayed commit or payout a no-op.
    mail_receipts: std::sync::Mutex<Vec<(u64, u64)>>,
    /// The mail-escrow step to fail on THIS database — a gateway killed before that step's
    /// transaction committed. `transfer`'s `kill_at` for the mail drive, and a `Mutex` because a
    /// re-drive test has to bring the database back up before driving again.
    mail_kill_at: std::sync::Mutex<Option<String>>,
    /// When set, `realm_group_op(ACCEPT, …)` fails with this message. INJECTED because a real
    /// one cannot be staged synchronously: every accept-time refusal the module has (already grouped,
    /// party full, the inviter no longer leads) needs the party to change BETWEEN the invite and the
    /// accept, and the gateway's bot answer runs in the same call as the invite. The failure is still
    /// reachable in production — a concurrent op on another socket — and what it must not do is leave
    /// the invite dialog hanging.
    party_accept_error: Option<String>,
    /// The transfer step to fail at, simulating a gateway killed before that step's
    /// transaction committed. `None` = nothing fails.
    kill_at: Option<String>,
    /// When set, `settle_home_shard` fails with this message (a transfer that could not be
    /// driven — an unreachable destination shard, a refused import).
    settle_error: Option<String>,
    /// How many `settle_home_shard` calls SUCCEED before `settle_error` starts firing. 0
    /// (derive-Default) = the very first one fails, i.e. the login-time failure the transfer
    /// test drives.
    /// 1 = the login routes fine and the WORLD-PORT's settle is the one that cannot be driven —
    /// the case that hung a real client on its loading screen forever.
    settle_ok_calls: usize,
    /// How many times `settle_home_shard` has been asked (drives `settle_ok_calls`).
    settle_calls: std::sync::atomic::AtomicUsize,
    /// Accounts `bind_shard_session` was called for, per shard.
    bound_sessions: std::sync::Mutex<Vec<u64>>,
    /// The REALM-CORE character→shard index this handle's `publish_shard_index` writes. In
    /// production that write goes to a third database (`realm_core()`); here it is just a map, so a
    /// test can assert the drive published the destination it settled on.
    realm_index: std::sync::Mutex<Vec<(u64, u32, u64)>>,
    /// When set, `publish_shard_index` fails with this message — an unreachable realm-core.
    publish_error: Option<String>,
    /// When set, every `movement_update` fails with this message. The case that matters is
    /// `"mover not in world"` — the module's answer for a packet that arrives after
    /// `teleport_player` despawned the entity, i.e. the tail of every cross-map port.
    movement_error: Option<String>,
    // Test recorder: the tuple is `realm_loot_op`'s argument list verbatim.
    #[allow(clippy::type_complexity)]
    /// Recorded `realm_loot_op` calls — `(op, corpse_guid, slot, item_entry, actor_guid, vote,
    /// deadline_micros, recipients)` — every arg the gateway's loot-roll routing/relay passed. The
    /// realm handle owns this; a world shard's staying empty is how a test tells "the vote/promotion
    /// went to the authority" from "it stayed shard-local".
    realm_loot_ops: std::sync::Mutex<Vec<(u8, u64, u8, u32, u64, u8, i64, Vec<u64>)>>,
    /// When set, `realm_loot_op` fails with this message.
    realm_loot_op_error: Option<String>,
    /// This WORLD SHARD's staging rolls `pending_local_rolls` answers — the relay's promotion
    /// INPUT. `Mutex`-wrapped (like `mirror`/`realm_whispers`) so a test can set it AFTER the fixture
    /// is wrapped in an `Arc` — every existing party/whisper topology builder hands back `Arc`s.
    /// Empty (derive-Default) = nothing to promote, byte-identical to before this field existed.
    pending_rolls: std::sync::Mutex<Vec<super::loot::PendingLootRoll>>,
    /// Recorded `settle_loot_roll` calls on THIS shard — `(corpse_guid, slot, winner_guid)`.
    settled_rolls: std::sync::Mutex<Vec<(u64, u8, u64)>>,
    /// When set, `settle_loot_roll` fails with this message.
    settle_loot_roll_error: Option<String>,
    /// Recorded `clear_promoted_loot_roll` calls on THIS shard — the roll ids the relay told
    /// this shard's staging copy to forget after a successful promotion.
    cleared_rolls: std::sync::Mutex<Vec<u64>>,
    /// This REALM-CORE handle's fixture `ROLL_WON` queue — `(corpse_guid, slot, winner_guid)`
    /// triples, in the order they "arrived". `loot_won_since(after_id)` answers every entry whose
    /// 1-based INDEX exceeds `after_id`, and the new watermark is the queue's length — the same
    /// shape the real `game_group_event.id` high-water mark has, without needing a fake event table.
    /// `Mutex`-wrapped for the same after-`Arc`-construction reason as `pending_rolls`.
    won_events: std::sync::Mutex<Vec<(u64, u8, u64)>>,
    /// Recorded `move_item` calls — `(from_slot, to_slot)`. Backs both CMSG_SWAP_INV_ITEM
    /// (drag within the main inventory) and CMSG_SWAP_ITEM (cross-bag, main-bag-only in this
    /// gateway), which both route onto this one store method.
    moved_items: std::sync::Mutex<Vec<(u8, u8)>>,
    /// Recorded `auto_bank_item` slots — backs both CMSG_AUTOBANK_ITEM (deposit) and
    /// CMSG_AUTOSTORE_BANK_ITEM (withdraw), which both route onto this one store method.
    auto_banked_items: std::sync::Mutex<Vec<u8>>,
    /// Recorded `buy_bank_slot` calls — the banker guid named on each `CMSG_BUY_BANK_SLOT`.
    bought_bank_slots: std::sync::Mutex<Vec<u64>>,
    /// Recorded `unequip_item` slots — the CMSG_AUTOSTORE_BAG_ITEM (right-click an equipped
    /// item) dispatch.
    unequipped_slots: std::sync::Mutex<Vec<u8>>,
    /// Recorded `sell_item` calls — `(vendor_guid, slot)`, AFTER the gateway resolves the
    /// wire's item INSTANCE guid to an inventory slot via `player_items`.
    sold_items: std::sync::Mutex<Vec<(u64, u8)>>,
    /// Recorded `repair_item` calls — `(npc_guid, slot)`. `slot == u8::MAX` is the
    /// "repair all" fixture (item guid 0 on the wire).
    repaired_items: std::sync::Mutex<Vec<(u64, u8)>>,
    /// Trainer rows `trainer_list` returns for ANY (player, trainer) pair — the
    /// CMSG_TRAINER_LIST fixture. Empty by default (an empty trainer window).
    trainer_spells: Vec<codec::TrainerSpellView>,
    /// Recorded `repop` calls — the caller's self_guid, one entry per CMSG_REPOP_REQUEST.
    repopped: std::sync::Mutex<Vec<u64>>,
    /// Recorded `reclaim_corpse` calls — `(self_guid, corpse_guid)` off CMSG_RECLAIM_CORPSE.
    reclaimed_corpses: std::sync::Mutex<Vec<(u64, u64)>>,
    /// Recorded `resurrect_response` calls — `(self_guid, accept)`, pinning the wire's
    /// `status != 0` → bool mapping.
    resurrect_responses: std::sync::Mutex<Vec<(u64, bool)>>,
    /// Recorded `spirit_healer_res` calls — `(self_guid, healer_guid)` off
    /// CMSG_SPIRIT_HEALER_ACTIVATE.
    spirit_healer_calls: std::sync::Mutex<Vec<(u64, u64)>>,
    /// Recorded `set_target` target guids — CMSG_SET_SELECTION. (`rec("set_target")` already
    /// pins the per-shard call NAME; this pins the ARGUMENT actually threaded through.)
    selected_targets: std::sync::Mutex<Vec<u64>>,
    /// When set, `set_target` fails before the reducer can complete. This models the call pipe
    /// whose transport dies while an admitted world session is in flight.
    set_target_error: Option<String>,
    /// Whether the in-world relay registration was torn down when the session ended.
    relay_stopped: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// Recorded `cancel_aura` spell ids — CMSG_CANCEL_AURA.
    cancelled_auras: std::sync::Mutex<Vec<u32>>,
    /// Recorded `cancel_cast` self_guids — CMSG_CANCEL_CAST.
    cancelled_casts: std::sync::Mutex<Vec<u64>>,
    /// `game_item_instance` on THIS database: `item_guid -> (owner_guid, snapshot)`. The mail
    /// attachment path deletes from here at send and inserts at take, which is the whole "a fenced
    /// item is in nobody's bags" property.
    mail_items: std::sync::Mutex<Vec<(u64, u64, mail::AttachedItem)>>,
    /// This shard's bags have no room — the fixture behind the full-bag refusal on a take.
    /// Atomic so a test can flip it AFTER the fixture is wrapped in an `Arc`, like `purses`.
    bags_full: std::sync::atomic::AtomicBool,
    /// What `player_items` returns for ANY owner guid — the CMSG_SELL_ITEM/CMSG_REPAIR_ITEM
    /// item-instance-guid → inventory-slot resolution fixture. Empty by default (no items), matching
    /// every earlier test that never sets this.
    player_items_fixture: Vec<codec::ItemInstanceView>,
    /// Recorded `initiate_trade` calls — `(self_guid, target_guid)` off CMSG_INITIATE_TRADE (#120).
    initiated_trades: std::sync::Mutex<Vec<(u64, u64)>>,
    /// Recorded `begin_trade` self_guids — CMSG_BEGIN_TRADE (#120).
    begun_trades: std::sync::Mutex<Vec<u64>>,
    /// Recorded `cancel_trade` self_guids — CMSG_CANCEL_TRADE (#120).
    cancelled_trades: std::sync::Mutex<Vec<u64>>,
    /// Recorded `set_trade_item` calls — `(self_guid, trade_slot, inv_slot)` AFTER the gateway's
    /// (bag, slot) → absolute-slot mapping (#121).
    set_trade_items: std::sync::Mutex<Vec<(u64, u8, u8)>>,
    /// Recorded `clear_trade_item` calls — `(self_guid, trade_slot)` (#121).
    cleared_trade_items: std::sync::Mutex<Vec<(u64, u8)>>,
    /// Recorded `set_trade_gold` calls — `(self_guid, copper)` after the wire's Gold decode (#121).
    set_trade_golds: std::sync::Mutex<Vec<(u64, u32)>>,
    /// Recorded `accept_trade` self_guids — CMSG_ACCEPT_TRADE (#122).
    accepted_trades: std::sync::Mutex<Vec<u64>>,
    /// Recorded `unaccept_trade` self_guids — CMSG_UNACCEPT_TRADE (#122).
    unaccepted_trades: std::sync::Mutex<Vec<u64>>,
    /// Recorded `busy_trade` self_guids — CMSG_BUSY_TRADE (#123).
    busy_trades: std::sync::Mutex<Vec<u64>>,
    /// Recorded `ignore_trade` self_guids — CMSG_IGNORE_TRADE (#123).
    ignore_trades: std::sync::Mutex<Vec<u64>>,
}

impl InMemoryStore {
    /// Record one player-scoped call against THIS handle's shard.
    fn rec(&self, what: &str) {
        self.calls
            .lock()
            .unwrap()
            .push((self.shard.clone(), what.to_string()));
    }

    /// One mail-escrow step boundary. `Err` is the gateway dying before this step committed: the
    /// call never lands, and nothing after it in the drive runs either.
    fn mail_kill(&self, step: &str) -> Result<()> {
        if self.mail_kill_at.lock().unwrap().as_deref() == Some(step) {
            anyhow::bail!("injected: the gateway died before {step} on {}", self.shard);
        }
        Ok(())
    }

    /// Take `copper` out of a purse on THIS database, or refuse and take nothing — the module's
    /// `charge_postage`, including its "no live entity here" arm for a guid with no purse row.
    fn debit(&self, guid: u64, copper: u32) -> Result<()> {
        let mut purses = self.purses.lock().unwrap();
        match purses.iter_mut().find(|(g, _)| *g == guid) {
            Some((_, money)) if *money >= copper => {
                *money -= copper;
                Ok(())
            }
            _ => Err(anyhow!(lyracore_shared::mail::NOT_ENOUGH_MONEY)),
        }
    }

    fn credit(&self, guid: u64, copper: u32) {
        let mut purses = self.purses.lock().unwrap();
        if let Some((_, money)) = purses.iter_mut().find(|(g, _)| *g == guid) {
            *money = money.saturating_add(copper);
        }
    }

    /// The module's `detach_item`: the mailable verdict, then the DELETE. `item_guid` 0 is a letter
    /// with no attachment.
    fn detach(&self, sender_guid: u64, item_guid: u64) -> Result<mail::AttachedItem> {
        if item_guid == 0 {
            return Ok(mail::AttachedItem::default());
        }
        let mut items = self.mail_items.lock().unwrap();
        match items
            .iter()
            .position(|(g, owner, _)| *g == item_guid && *owner == sender_guid)
        {
            None => Err(anyhow!(lyracore_shared::mail::NOT_YOUR_ITEM)),
            Some(i) if items[i].2.soulbound => {
                Err(anyhow!(lyracore_shared::mail::ITEM_IS_SOULBOUND))
            }
            Some(i) => Ok(items.remove(i).2),
        }
    }

    /// The module's `store_instance_state`: one new row carrying the recorded state, or the item
    /// module's own full-bag refusal.
    fn store_snapshot(&self, owner_guid: u64, item: &mail::AttachedItem) -> Result<()> {
        if self.bags_full.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(anyhow!(lyracore_shared::mail::INVENTORY_FULL));
        }
        let mut items = self.mail_items.lock().unwrap();
        let guid = items.iter().map(|(g, _, _)| *g).max().unwrap_or(0) + 1;
        items.push((guid, owner_guid, item.clone()));
        Ok(())
    }

    /// Every item `owner` holds on this database — the assertion surface for "it left the bags",
    /// "it arrived unchanged" and "it never arrived twice".
    pub(crate) fn bags_of(&self, owner: u64) -> Vec<mail::AttachedItem> {
        self.mail_items
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, o, _)| *o == owner)
            .map(|(_, _, i)| i.clone())
            .collect()
    }

    /// The module's `insert_mail`: the row both write paths reach, so a letter written by the
    /// single-database send and one written by the escrow's commit cannot differ.
    #[allow(clippy::too_many_arguments)]
    fn write_mail(
        &self,
        sender_guid: u64,
        recipient_guid: u64,
        subject: String,
        body: String,
        money: u32,
        cod: u32,
        item: &mail::AttachedItem,
    ) {
        let mut mails = self.mails.lock().unwrap();
        let id = mails.iter().map(|(_, m)| m.id).max().unwrap_or(0) + 1;
        mails.push((
            recipient_guid,
            codec::MailView {
                id,
                sender_guid,
                subject,
                body,
                money,
                cod,
                item_entry: item.entry,
                item_stack_count: item.stack_count,
                item_durability: item.durability,
                item_enchant_id: item.enchant_id,
                item_soulbound: item.soulbound,
                created_at_secs: 1_000,
                ..Default::default()
            },
        ));
    }

    /// Record a transfer step and honour an injected kill. `Err` means "the gateway died
    /// before this step's transaction committed", which is exactly a truncated drive.
    fn xstep(&self, what: &str) -> Result<&std::sync::Arc<FakeShardDb>> {
        let db = self
            .xdb
            .as_ref()
            .ok_or_else(|| anyhow!("this store does not implement cross-database transfers"))?;
        if self.kill_at.as_deref() == Some(what) {
            return Err(anyhow!("gateway killed at {what}"));
        }
        self.rec(what);
        Ok(db)
    }
}

impl WorldStore for InMemoryStore {
    fn home_shard(&self, _character_guid: u64) -> Option<std::sync::Arc<dyn WorldStore>> {
        let nth = self
            .home_shard_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let resolved = match (&self.home_after_flip, nth) {
            (Some(flipped), n) if n >= 1 => Some(flipped.clone()),
            _ => self.home.clone(),
        };
        resolved.map(|h| h as std::sync::Arc<dyn WorldStore>)
    }
    fn shard_name(&self) -> &str {
        &self.shard
    }

    // --- The escrow protocol, with the MODULE's guards reproduced. A permissive mock would
    // --- let every ordering mutation in `run_transfer` pass; these are what make the order matter.

    fn settle_home_shard(
        &self,
        character_guid: u64,
    ) -> Result<Option<std::sync::Arc<dyn WorldStore>>> {
        if let Some(e) = &self.settle_error {
            let nth = self
                .settle_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if nth >= self.settle_ok_calls {
                return Err(anyhow!("{e}"));
            }
        }
        Ok(self.home_shard(character_guid))
    }

    fn bind_shard_session(&self, account_id: u64, _session_key: &[u8; 40]) -> Result<()> {
        self.rec("bind_shard_session");
        self.bound_sessions.lock().unwrap().push(account_id);
        Ok(())
    }

    fn escrowed_transfer(&self, character_guid: u64) -> Option<super::transfer::EscrowedTransfer> {
        let db = self.xdb.as_ref()?;
        let out = lk(&db.out_rows);
        out.values()
            .find(|e| e.character_guid == character_guid)
            .map(|e| super::transfer::EscrowedTransfer {
                transfer_id: e.transfer_id,
                character_guid: e.character_guid,
                dest_map_id: e.dest_map_id,
                dest_instance_id: e.dest_instance_id,
                blob: e.blob.clone(),
            })
    }

    fn character_destination(&self, character_guid: u64) -> Option<super::transfer::TransferPlan> {
        let c = self.xdb.as_ref()?.get(character_guid)?;
        Some(super::transfer::TransferPlan {
            transfer_id: super::transfer::transfer_id_for(character_guid),
            character_guid,
            dest_map_id: c.map_id,
            dest_instance_id: c.instance_id,
            dest_x: 0.0,
            dest_y: 0.0,
            dest_z: 0.0,
            dest_o: 0.0,
        })
    }

    /// `plan_begin`: replay on a matching escrow, refuse without a source copy, refuse a second
    /// escrow for the same character, otherwise freeze + serialize.
    ///
    /// The `escrowed_guid` lookup is the OUT-row with the IN-row as a fallback, exactly as
    /// `module/src/transfer/mod.rs`'s `begin_transfer` computes it — and that fallback is load-bearing
    /// now the transfer id IS the character guid: a database holding an unreleased ARRIVAL in-row
    /// for this character answers `BeginPlan::Replay` to a genuine new transfer, i.e. reports
    /// success while freezing nothing. A mock that only looked at `out_rows` could not see it.
    fn begin_transfer(&self, plan: &super::transfer::TransferPlan) -> Result<()> {
        let db = self.xstep("begin_transfer")?;
        let mut out = lk(&db.out_rows);
        let escrowed_guid = out
            .get(&plan.transfer_id)
            .map(|e| e.character_guid)
            .or_else(|| lk(&db.in_rows).get(&plan.transfer_id).copied());
        if let Some(existing) = escrowed_guid {
            if existing != plan.character_guid {
                return Err(anyhow!("transfer id collision"));
            }
            return Ok(()); // BeginPlan::Replay
        }
        let Some(c) = db.get(plan.character_guid) else {
            return Err(anyhow!("no such character: {}", plan.character_guid));
        };
        if out
            .values()
            .any(|e| e.character_guid == plan.character_guid)
        {
            return Err(anyhow!(
                "character {} is already in transit",
                plan.character_guid
            ));
        }
        out.insert(
            plan.transfer_id,
            FakeEscrow {
                transfer_id: plan.transfer_id,
                character_guid: plan.character_guid,
                dest_map_id: plan.dest_map_id,
                dest_instance_id: plan.dest_instance_id,
                blob: fake_blob(
                    plan.character_guid,
                    plan.dest_map_id,
                    plan.dest_instance_id,
                    &c.payload,
                ),
            },
        );
        Ok(())
    }

    /// `import_character_blob`: replay on the in-row PK, refuse to land on a LIVE character,
    /// otherwise materialise the row + its payload at the escrow's destination.
    fn import_character_blob(&self, transfer_id: u64, blob: &[u8]) -> Result<()> {
        let db = self.xstep("import_character_blob")?;
        let (guid, arriving) = parse_blob(blob);
        // NOTE: every `in_rows` guard below is scoped and dropped before `db.live()`, which locks
        // `in_rows` itself. `std::sync::Mutex` is not re-entrant, so holding one across that call
        // self-deadlocks — and a deadlock makes an ordering mutation HANG the suite instead of
        // turning a named test red, which is a coverage failure wearing a hang's clothes.
        let replayed = lk(&db.in_rows).get(&transfer_id).copied();
        if let Some(existing) = replayed {
            if existing != guid {
                return Err(anyhow!(
                    "transfer id already imported for another character"
                ));
            }
            return Ok(());
        }
        if db.live(guid) {
            return Err(anyhow!("character {guid} is already live on this shard"));
        }
        // The destination rides IN the blob, exactly as the real `ExportBlob`'s `dest_*` fields do:
        // cross-database the blob is the only thing that reaches this side.
        lk(&db.characters).insert(guid, arriving);
        lk(&db.in_rows).insert(transfer_id, guid);
        Ok(())
    }

    /// `confirm_import`: the SOURCE-side attestation. Refuses without a local escrow.
    fn confirm_import(&self, transfer_id: u64) -> Result<()> {
        let db = self.xstep("confirm_import")?;
        let out = lk(&db.out_rows);
        let Some(escrow) = out.get(&transfer_id) else {
            return Err(anyhow!(
                "transfer {transfer_id}: nothing escrowed here to confirm"
            ));
        };
        lk(&db.in_rows).insert(transfer_id, escrow.character_guid);
        Ok(())
    }

    /// `plan_finish`: refuses while the in-row (the attestation) is absent — the guard that makes
    /// "zero durable copies" unreachable — then cascade-deletes the source copy, escrow last.
    fn finish_transfer(&self, transfer_id: u64) -> Result<()> {
        let db = self.xstep("finish_transfer")?;
        let mut out = lk(&db.out_rows);
        let Some(escrow) = out.get(&transfer_id).cloned() else {
            return Ok(()); // FinishPlan::AlreadyDone
        };
        if !lk(&db.in_rows).contains_key(&transfer_id) {
            return Err(anyhow!(
                "transfer {transfer_id}: not imported — refusing to release"
            ));
        }
        lk(&db.characters).remove(&escrow.character_guid);
        lk(&db.in_rows).remove(&transfer_id);
        out.remove(&transfer_id);
        Ok(())
    }

    /// `release_transfer`: refuses on a shard that is the SOURCE; replay-safe otherwise.
    fn release_transfer(&self, transfer_id: u64) -> Result<()> {
        let Some(_) = self.xdb.as_ref() else {
            return Ok(());
        };
        let db = self.xstep("release_transfer")?;
        if lk(&db.out_rows).contains_key(&transfer_id) {
            return Err(anyhow!(
                "transfer {transfer_id}: this database holds the SOURCE out-row"
            ));
        }
        lk(&db.in_rows).remove(&transfer_id);
        Ok(())
    }

    /// The realm-core index publish. Recorded in the shared call log so its POSITION in the
    /// drive is assertable, not just its effect.
    fn publish_shard_index(
        &self,
        character_guid: u64,
        map_id: u32,
        instance_id: u64,
    ) -> Result<()> {
        if let Some(e) = &self.publish_error {
            return Err(anyhow!("{e}"));
        }
        // Through `xstep`, like every other step of the drive — NOT a bare `rec`. Every other
        // transfer method routes its "gateway killed here" injection through it, and this one
        // originally did not, so `kill_at = "publish_shard_index"` was silently inert and the
        // crash matrix reported a PASS for a boundary it never killed at.
        self.xstep("publish_shard_index")?;
        self.realm_index
            .lock()
            .unwrap()
            .push((character_guid, map_id, instance_id));
        Ok(())
    }

    fn ensure_instance(&self, instance_id: u64, _map_id: u32, _party_id: u64) -> Result<()> {
        let db = self.xstep("ensure_instance")?;
        if instance_id == 0 {
            return Err(anyhow!("instance 0 is the open world"));
        }
        // The module's own shape: a mirror of an instance that is ALREADY here joins it (early
        // return) instead of spawning a second population. `HashSet::insert` reports that for free,
        // and the count is what the second-party-member test asserts against.
        if lk(&db.instances).insert(instance_id) {
            lk(&db.populated).push(instance_id);
        }
        Ok(())
    }

    fn evict_instance_population(&self, instance_id: u64) -> Result<()> {
        let db = self.xstep("evict_instance_population")?;
        if instance_id == 0 {
            return Err(anyhow!("instance 0 is the open world"));
        }
        lk(&db.evicted).push(instance_id);
        Ok(())
    }

    fn lookup_session(&self, account_name: &str) -> Result<Option<WorldSession>> {
        Ok((account_name == self.username)
            .then(|| self.session.clone())
            .flatten())
    }
    fn characters(&self, _account_id: u64) -> Result<Vec<codec::CharacterView>> {
        self.rec("characters");
        let mut out = self.characters.clone();
        out.extend(self.created_characters.lock().unwrap().iter().cloned());
        Ok(out)
    }
    fn create_character(
        &self,
        _account_id: u64,
        name: &str,
        race: u8,
        class: u8,
        _gender: u8,
        _appearance: codec::Appearance,
    ) -> Result<codec::CharCreateOutcome> {
        // Fake: a name already among the seeded OR previously created characters is "in use",
        // else success — and a success actually RECORDS the character, assigning it a real
        // guid `characters()` then unions in. Before a review caught it, this call was a pure
        // no-op the fake immediately forgot, which let a test claim to drive "the character
        // CREATE just produced" while actually logging into an unrelated hardcoded/pre-seeded guid.
        if self.characters.iter().any(|c| c.name == name)
            || self
                .created_characters
                .lock()
                .unwrap()
                .iter()
                .any(|c| c.name == name)
        {
            return Ok(codec::CharCreateOutcome::NameInUse);
        }
        let guid = 500
            + self
                .next_created_guid
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.created_characters
            .lock()
            .unwrap()
            .push(codec::CharacterView {
                guid,
                name: name.to_string(),
                race,
                class,
                level: 1,
                ..Default::default()
            });
        Ok(codec::CharCreateOutcome::Success)
    }
    fn delete_character(
        &self,
        account_id: u64,
        character_guid: u64,
    ) -> Result<codec::CharDeleteOutcome> {
        self.deleted
            .lock()
            .unwrap()
            .push((account_id, character_guid));
        Ok(self
            .delete_outcome
            .unwrap_or(codec::CharDeleteOutcome::Success))
    }
    fn player_login(&self, _account_id: u64, _character_guid: u64) -> Result<codec::EntityView> {
        self.rec("player_login");
        let call = self
            .login_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if call > 0 {
            if let Some(e) = &self.worldport_login_error {
                return Err(anyhow!("{e}"));
            }
            if let Some(e) = self.worldport_entity.clone() {
                return Ok(e);
            }
        }
        self.login_entity
            .clone()
            .ok_or_else(|| anyhow!("no login entity configured"))
    }
    fn movement_update(
        &self,
        _account_id: u64,
        _self_guid: u64,
        opcode: u32,
        info: &MovementInfo,
    ) -> Result<()> {
        self.rec("movement_update");
        if let Some(e) = &self.movement_error {
            return Err(anyhow!("movement_update reducer failed: {e}"));
        }
        self.moves.lock().unwrap().push((
            opcode,
            info.position.x,
            info.position.y,
            info.position.z,
            info.orientation,
            info.timestamp,
        ));
        Ok(())
    }
    fn subscribe_player_events(
        &self,
        _account_id: u64,
        self_guid: u64,
        _login_instance: u64,
        login_map: u32,
        login_x: f32,
        login_y: f32,
        tx: SessionTx,
    ) -> Result<PlayerSubscriptions> {
        self.rec("subscribe_player_events");
        self.subscribed
            .lock()
            .unwrap()
            .push((self_guid, login_map, login_x, login_y));
        *self.session_depth.lock().unwrap() = Some(tx.depth_handle());
        match &self.relay_stopped {
            Some(stopped) => Ok(PlayerSubscriptions::with_teardown({
                let stopped = stopped.clone();
                move || stopped.store(true, std::sync::atomic::Ordering::SeqCst)
            })),
            None => Ok(PlayerSubscriptions::empty()),
        }
    }
    fn logout(&self, _account_id: u64, _self_guid: u64) -> Result<()> {
        self.rec("logout");
        self.logout_called
            .store(true, std::sync::atomic::Ordering::SeqCst);
        match &self.logout_error {
            Some(e) => Err(anyhow!("{e}")),
            None => Ok(()),
        }
    }
    fn character_by_guid(&self, guid: u64) -> Result<Option<codec::CharacterView>> {
        Ok(self.characters.iter().find(|c| c.guid == guid).cloned())
    }
    fn creature_template(&self, _entry: u32) -> Result<Option<codec::CreatureView>> {
        Ok(None)
    }
    fn item_template(&self, _entry: u32) -> Result<Option<codec::ItemTemplateView>> {
        Ok(None)
    }
    fn gameobject_template(&self, _entry: u32) -> Result<Option<codec::GameObjectTemplateView>> {
        Ok(None)
    }
    fn use_gameobject(&self, _account_id: u64, _self_guid: u64, _go_guid: u64) -> Result<()> {
        Ok(())
    }
    fn client_command(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _cmd: String,
        _payload: String,
    ) -> Result<()> {
        Ok(())
    }

    fn enter_areatrigger(&self, _account_id: u64, _self_guid: u64, _trigger_id: u32) -> Result<()> {
        Ok(())
    }
    fn player_items(&self, _owner_guid: u64) -> Result<Vec<codec::ItemInstanceView>> {
        Ok(self.player_items_fixture.clone())
    }
    fn player_skills(&self, _character_guid: u64) -> Result<Vec<(u32, u16, u16)>> {
        Ok(Vec::new())
    }
    fn effective_armor(&self, _guid: u64) -> u32 {
        // No gear/auras in the test store → effective == the login entity's armor.
        self.login_entity
            .as_ref()
            .map(|e| e.effective_armor)
            .unwrap_or(0)
    }
    fn corpse_loot(&self, _corpse_guid: u64, viewer_guid: u64) -> Result<Vec<codec::LootItemView>> {
        Ok(self
            .corpse_loot_by_viewer
            .get(&viewer_guid)
            .cloned()
            .unwrap_or_default())
    }
    fn vendor_items(&self, _vendor_guid: u64) -> Result<Vec<codec::VendorItemView>> {
        Ok(self.vendor_stock.clone())
    }
    fn npc_refuses_interaction(&self, _npc_guid: u64, _player_guid: u64) -> Result<bool> {
        Ok(self.npc_refuses) // default false — every existing fixture NPC keeps interacting
    }
    fn mail_list(&self, recipient_guid: u64) -> Result<Vec<codec::MailView>> {
        self.rec("mail_list");
        Ok(self
            .mails
            .lock()
            .unwrap()
            .iter()
            .filter(|(to, _)| *to == recipient_guid)
            .map(|(_, m)| m.clone())
            .collect())
    }
    fn mailbox_in_range(&self, mailbox_guid: u64, _player_guid: u64) -> Result<bool> {
        self.rec("mailbox_in_range");
        Ok(self.mailboxes.contains(&mailbox_guid))
    }
    /// Models the module's `apply_mark_read`: the row lookup scoped to `recipient_guid` IS the
    /// authorization, so a mail that exists but belongs to someone else fails the same way a
    /// nonexistent id does.
    fn mail_mark_read(&self, recipient_guid: u64, mail_id: u64) -> Result<()> {
        self.rec("mail_mark_read");
        let mut mails = self.mails.lock().unwrap();
        match mails
            .iter_mut()
            .find(|(to, m)| *to == recipient_guid && m.id == mail_id)
        {
            Some((_, m)) => {
                m.was_read = true;
                Ok(())
            }
            None => Err(anyhow!(lyracore_shared::mail::NOT_YOUR_MAIL)),
        }
    }
    /// Models the module's `apply_delete`: same merged not-found/not-yours refusal as mark-read.
    fn mail_delete(&self, recipient_guid: u64, mail_id: u64) -> Result<()> {
        self.rec("mail_delete");
        let mut mails = self.mails.lock().unwrap();
        let before = mails.len();
        mails.retain(|(to, m)| !(*to == recipient_guid && m.id == mail_id));
        if mails.len() == before {
            return Err(anyhow!(lyracore_shared::mail::NOT_YOUR_MAIL));
        }
        Ok(())
    }
    /// Models the module's `apply_return`: the SAME row, re-addressed to whoever sent it, with
    /// whatever it still carries (or nothing) travelling unchanged — except the cash-on-delivery
    /// price, which is dropped, because the row is going back to whoever set it.
    fn mail_return(&self, recipient_guid: u64, mail_id: u64) -> Result<()> {
        self.rec("mail_return");
        let mut mails = self.mails.lock().unwrap();
        match mails
            .iter_mut()
            .find(|(to, m)| *to == recipient_guid && m.id == mail_id)
        {
            Some((to, m)) => {
                let sender = m.sender_guid;
                m.sender_guid = recipient_guid;
                m.was_read = false;
                m.cod = 0;
                *to = sender;
                Ok(())
            }
            None => Err(anyhow!(lyracore_shared::mail::NOT_YOUR_MAIL)),
        }
    }
    /// Models the module's `apply_send`: the postage plus the attached coin leave the purse and the
    /// row is written, in ONE call — the single-database plane's one transaction. The id is
    /// per-database, as the module's `auto_inc` is.
    #[allow(clippy::too_many_arguments)]
    fn mail_send(
        &self,
        sender_guid: u64,
        recipient_guid: u64,
        subject: String,
        body: String,
        money: u32,
        cod: u32,
        item_guid: u64,
    ) -> Result<()> {
        self.rec("mail_send");
        let item = self.detach(sender_guid, item_guid)?;
        self.debit(sender_guid, lyracore_shared::mail::total_cost(money))?;
        self.sent_mail.lock().unwrap().push((
            sender_guid,
            recipient_guid,
            subject.clone(),
            body.clone(),
            money,
        ));
        self.write_mail(
            sender_guid,
            recipient_guid,
            subject,
            body,
            money,
            cod,
            &item,
        );
        Ok(())
    }

    /// Models the module's `apply_take_item`: the COD debit, the grant, the clear and the seller's
    /// payout row are ONE transaction, so a full bag or a price the taker cannot pay leaves the
    /// letter exactly as it was, and a second take finds an empty one.
    fn mail_take_item(&self, recipient_guid: u64, mail_id: u64) -> Result<()> {
        let (item, settlement) = {
            let mails = self.mails.lock().unwrap();
            let Some((_, m)) = mails
                .iter()
                .find(|(to, m)| *to == recipient_guid && m.id == mail_id)
            else {
                return Err(anyhow!(lyracore_shared::mail::NOT_YOUR_MAIL));
            };
            if m.item_entry == 0 {
                return Err(anyhow!(lyracore_shared::mail::NOTHING_TO_TAKE));
            }
            (
                mail::AttachedItem {
                    entry: m.item_entry,
                    stack_count: m.item_stack_count,
                    durability: m.item_durability,
                    enchant_id: m.item_enchant_id,
                    soulbound: m.item_soulbound,
                },
                lyracore_shared::mail::cod_settlement(
                    m.cod,
                    m.sender_guid,
                    &m.subject,
                    recipient_guid,
                ),
            )
        };
        self.rec("mail_take_item");
        if let Some(s) = &settlement {
            self.debit(s.payer_guid, s.copper)
                .map_err(|_| anyhow!(lyracore_shared::mail::COD_NOT_AFFORDABLE))?;
        }
        if let Err(e) = self.store_snapshot(recipient_guid, &item) {
            // The fake cannot roll back, so it undoes the one write it made — the real module gets
            // this from the transaction, and asserting on it is the point of the full-bag test.
            if let Some(s) = &settlement {
                self.credit(s.payer_guid, s.copper);
            }
            return Err(e);
        }
        let mut mails = self.mails.lock().unwrap();
        if let Some((_, m)) = mails
            .iter_mut()
            .find(|(to, m)| *to == recipient_guid && m.id == mail_id)
        {
            m.item_entry = 0;
            m.item_stack_count = 0;
            m.item_durability = 0;
            m.item_enchant_id = 0;
            m.item_soulbound = false;
            m.cod = 0;
        }
        drop(mails);
        if let Some(s) = settlement {
            self.write_mail(
                s.payer_guid,
                s.payee_guid,
                s.subject,
                String::new(),
                s.copper,
                0,
                &mail::AttachedItem::default(),
            );
        }
        Ok(())
    }

    fn mail_item_room(&self, _payee_guid: u64) -> Result<()> {
        self.rec("mail_item_room");
        if self.bags_full.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(anyhow!(lyracore_shared::mail::INVENTORY_FULL));
        }
        Ok(())
    }
    /// Models the module's `apply_take_money`: the credit and the clear are one transaction, so a
    /// second take finds an empty row.
    fn mail_take_money(&self, recipient_guid: u64, mail_id: u64) -> Result<()> {
        self.rec("mail_take_money");
        let money = {
            let mut mails = self.mails.lock().unwrap();
            let Some((_, m)) = mails
                .iter_mut()
                .find(|(to, m)| *to == recipient_guid && m.id == mail_id)
            else {
                return Err(anyhow!(lyracore_shared::mail::NOT_YOUR_MAIL));
            };
            if m.money == 0 {
                return Err(anyhow!(lyracore_shared::mail::NOTHING_TO_TAKE));
            }
            std::mem::take(&mut m.money)
        };
        self.credit(recipient_guid, money);
        Ok(())
    }
    /// Models `mail_escrow::apply_fence`: the whole cost leaves the purse into a fence row here.
    fn mail_fence(
        &self,
        escrow_id: u64,
        sender_guid: u64,
        recipient_guid: u64,
        subject: String,
        body: String,
        money: u32,
        postage: u32,
        item_guid: u64,
        cod: u32,
        cod_source_mail_id: u64,
    ) -> Result<()> {
        self.rec("mail_fence");
        self.mail_kill("mail_fence")?;
        let escrows = self.mail_escrows.lock().unwrap();
        if escrows.iter().any(|(_, e)| e.escrow_id == escrow_id) {
            return Ok(()); // replay — the purse must not be debited twice for one letter
        }
        drop(escrows);
        // The attachment before the debit: it is the refusal that can still fire, and nothing has
        // been written when it does.
        let item = self.detach(sender_guid, item_guid)?;
        self.debit(sender_guid, money.saturating_add(postage))?;
        self.mail_escrows.lock().unwrap().push((
            sender_guid,
            mail::HeldEscrow {
                escrow_id,
                recipient_guid,
                subject,
                body,
                money,
                postage,
                payout: false,
                mail_id: cod_source_mail_id,
                item,
                cod,
            },
        ));
        self.attested.lock().unwrap().push((escrow_id, false));
        Ok(())
    }
    /// Models `mail_escrow::apply_commit`: the row plus a receipt, idempotent on the escrow id.
    fn mail_commit(
        &self,
        escrow_id: u64,
        sender_guid: u64,
        recipient_guid: u64,
        subject: String,
        body: String,
        money: u32,
        item: mail::AttachedItem,
        cod: u32,
        cod_source_mail_id: u64,
    ) -> Result<()> {
        self.rec("mail_commit");
        self.mail_kill("mail_commit")?;
        let mut receipts = self.mail_receipts.lock().unwrap();
        if receipts.iter().any(|(id, _)| *id == escrow_id) {
            return Ok(());
        }
        receipts.push((escrow_id, recipient_guid));
        drop(receipts);
        self.sent_mail.lock().unwrap().push((
            sender_guid,
            recipient_guid,
            subject.clone(),
            body.clone(),
            money,
        ));
        self.write_mail(
            sender_guid,
            recipient_guid,
            subject,
            body,
            money,
            cod,
            &item,
        );
        // The price stops being owed in the SAME call that delivers the payment for it — the
        // module clears it inside the commit's transaction, which is what makes a COD take charge
        // once however the drive is interrupted.
        if cod_source_mail_id != 0 {
            if let Some((_, m)) = self
                .mails
                .lock()
                .unwrap()
                .iter_mut()
                .find(|(_, m)| m.id == cod_source_mail_id)
            {
                m.cod = 0;
            }
        }
        Ok(())
    }
    /// Models `mail_escrow::apply_take_fence`: the copper leaves the ROW into a fence here.
    fn mail_take_money_fence(
        &self,
        escrow_id: u64,
        payee_guid: u64,
        mail_id: u64,
        expect_money: u32,
    ) -> Result<()> {
        self.rec("mail_take_money_fence");
        self.mail_kill("mail_take_money_fence")?;
        if self
            .mail_escrows
            .lock()
            .unwrap()
            .iter()
            .any(|(_, e)| e.escrow_id == escrow_id)
        {
            return Ok(());
        }
        let money = {
            let mut mails = self.mails.lock().unwrap();
            let Some((_, m)) = mails
                .iter_mut()
                .find(|(to, m)| *to == payee_guid && m.id == mail_id)
            else {
                return Err(anyhow!(lyracore_shared::mail::NOT_YOUR_MAIL));
            };
            if m.money == 0 {
                return Err(anyhow!(lyracore_shared::mail::NOTHING_TO_TAKE));
            }
            if m.money != expect_money {
                return Err(anyhow!(
                    "refusing to fence an amount the payout would not match"
                ));
            }
            std::mem::take(&mut m.money)
        };
        self.mail_escrows.lock().unwrap().push((
            payee_guid,
            mail::HeldEscrow {
                escrow_id,
                recipient_guid: payee_guid,
                subject: String::new(),
                body: String::new(),
                money,
                postage: 0,
                payout: true,
                mail_id,
                item: mail::AttachedItem::default(),
                cod: 0,
            },
        ));
        self.attested.lock().unwrap().push((escrow_id, false));
        Ok(())
    }
    /// Models `mail_escrow::apply_take_item_fence`: the ATTACHMENT leaves the row into a fence.
    fn mail_take_item_fence(
        &self,
        escrow_id: u64,
        payee_guid: u64,
        mail_id: u64,
        expect_entry: u32,
    ) -> Result<()> {
        self.rec("mail_take_item_fence");
        self.mail_kill("mail_take_item_fence")?;
        if self
            .mail_escrows
            .lock()
            .unwrap()
            .iter()
            .any(|(_, e)| e.escrow_id == escrow_id)
        {
            return Ok(());
        }
        let item = {
            let mut mails = self.mails.lock().unwrap();
            let Some((_, m)) = mails
                .iter_mut()
                .find(|(to, m)| *to == payee_guid && m.id == mail_id)
            else {
                return Err(anyhow!(lyracore_shared::mail::NOT_YOUR_MAIL));
            };
            if m.item_entry == 0 {
                return Err(anyhow!(lyracore_shared::mail::NOTHING_TO_TAKE));
            }
            if m.item_entry != expect_entry {
                return Err(anyhow!(
                    "refusing to fence an item the grant would not match"
                ));
            }
            let item = mail::AttachedItem {
                entry: m.item_entry,
                stack_count: m.item_stack_count,
                durability: m.item_durability,
                enchant_id: m.item_enchant_id,
                soulbound: m.item_soulbound,
            };
            m.item_entry = 0;
            m.item_stack_count = 0;
            m.item_durability = 0;
            m.item_enchant_id = 0;
            m.item_soulbound = false;
            item
        };
        self.mail_escrows.lock().unwrap().push((
            payee_guid,
            mail::HeldEscrow {
                escrow_id,
                recipient_guid: payee_guid,
                subject: String::new(),
                body: String::new(),
                money: 0,
                postage: 0,
                payout: true,
                mail_id,
                item,
                cod: 0,
            },
        ));
        self.attested.lock().unwrap().push((escrow_id, false));
        Ok(())
    }
    /// Models `mail_escrow::apply_item_payout`: the grant plus a receipt, idempotent on the escrow
    /// id, and refused by a full bag — which leaves the fence holding the item.
    fn mail_item_payout(
        &self,
        escrow_id: u64,
        payee_guid: u64,
        _mail_id: u64,
        item: mail::AttachedItem,
    ) -> Result<()> {
        self.rec("mail_item_payout");
        self.mail_kill("mail_item_payout")?;
        let receipts = self.mail_receipts.lock().unwrap();
        if receipts.iter().any(|(id, _)| *id == escrow_id) {
            return Ok(());
        }
        drop(receipts);
        self.store_snapshot(payee_guid, &item)?;
        self.mail_receipts
            .lock()
            .unwrap()
            .push((escrow_id, payee_guid));
        Ok(())
    }
    /// Models `mail_escrow::apply_payout`: the credit plus a receipt, idempotent on the escrow id.
    fn mail_payout(
        &self,
        escrow_id: u64,
        payee_guid: u64,
        _mail_id: u64,
        amount: u32,
    ) -> Result<()> {
        self.rec("mail_payout");
        self.mail_kill("mail_payout")?;
        let mut receipts = self.mail_receipts.lock().unwrap();
        if receipts.iter().any(|(id, _)| *id == escrow_id) {
            return Ok(());
        }
        if !self
            .purses
            .lock()
            .unwrap()
            .iter()
            .any(|(g, _)| *g == payee_guid)
        {
            return Err(anyhow!(lyracore_shared::mail::NOT_IN_WORLD));
        }
        receipts.push((escrow_id, payee_guid));
        drop(receipts);
        self.credit(payee_guid, amount);
        Ok(())
    }
    fn mail_confirm_delivery(&self, escrow_id: u64) -> Result<()> {
        self.rec("mail_confirm_delivery");
        self.mail_kill("mail_confirm_delivery")?;
        let mut attested = self.attested.lock().unwrap();
        match attested.iter_mut().find(|(id, _)| *id == escrow_id) {
            Some((_, done)) => {
                *done = true;
                Ok(())
            }
            None => Err(anyhow!("mail escrow {escrow_id}: nothing fenced here")),
        }
    }
    /// Models `mail_escrow::apply_settle`, delete-last included: it REFUSES while unattested.
    fn mail_settle(&self, escrow_id: u64) -> Result<()> {
        self.rec("mail_settle");
        self.mail_kill("mail_settle")?;
        let attested = self
            .attested
            .lock()
            .unwrap()
            .iter()
            .find(|(id, _)| *id == escrow_id)
            .map(|(_, done)| *done);
        match attested {
            None => Ok(()), // already settled, or this call reached the wrong database
            Some(false) => Err(anyhow!(
                "mail escrow {escrow_id}: delivery not attested — refusing to destroy the fence"
            )),
            Some(true) => {
                self.mail_escrows
                    .lock()
                    .unwrap()
                    .retain(|(_, e)| e.escrow_id != escrow_id);
                self.attested
                    .lock()
                    .unwrap()
                    .retain(|(id, _)| *id != escrow_id);
                Ok(())
            }
        }
    }
    fn mail_escrows_of(&self, sender_guid: u64) -> Result<Vec<mail::HeldEscrow>> {
        self.rec("mail_escrows_of");
        Ok(self
            .mail_escrows
            .lock()
            .unwrap()
            .iter()
            .filter(|(owner, _)| *owner == sender_guid)
            .map(|(_, e)| e.clone())
            .collect())
    }
    fn trainer_serves(&self, _player_guid: u64, _trainer_guid: u64) -> Result<bool> {
        Ok(!self.trainer_refuses_class) // default true — every existing fixture trainer serves
    }
    fn buy_item(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _vendor_guid: u64,
        _item_entry: u32,
        _count: u32,
    ) -> Result<()> {
        match &self.trade_error {
            Some(e) => Err(anyhow!("{e}")),
            None => Ok(()),
        }
    }
    fn sell_item(
        &self,
        _account_id: u64,
        _self_guid: u64,
        vendor_guid: u64,
        slot: u8,
    ) -> Result<()> {
        if let Some(e) = &self.trade_error {
            return Err(anyhow!("{e}"));
        }
        self.sold_items.lock().unwrap().push((vendor_guid, slot));
        Ok(())
    }
    fn buyback_item(
        &self,
        _account_id: u64,
        _self_guid: u64,
        vendor_guid: u64,
        slot: u8,
    ) -> Result<()> {
        if let Some(e) = &self.trade_error {
            return Err(anyhow!("{e}"));
        }
        self.bought_back.lock().unwrap().push((vendor_guid, slot));
        Ok(())
    }
    fn repair_item(
        &self,
        _account_id: u64,
        _self_guid: u64,
        npc_guid: u64,
        slot: u8,
    ) -> Result<()> {
        if let Some(e) = &self.trade_error {
            return Err(anyhow!("{e}"));
        }
        self.repaired_items.lock().unwrap().push((npc_guid, slot));
        Ok(())
    }
    fn trainer_list(
        &self,
        _player_guid: u64,
        _trainer_guid: u64,
    ) -> Result<Vec<codec::TrainerSpellView>> {
        Ok(self.trainer_spells.clone())
    }
    fn buy_trainer_spell(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _trainer_guid: u64,
        _spell_id: u32,
    ) -> Result<()> {
        match &self.trade_error {
            Some(e) => Err(anyhow!("{e}")),
            None => Ok(()),
        }
    }
    fn skin_corpse(&self, _account_id: u64, _self_guid: u64, corpse_guid: u64) -> Result<()> {
        if let Some(e) = &self.trade_error {
            return Err(anyhow!("{e}"));
        }
        self.skinned.lock().unwrap().push(corpse_guid);
        Ok(())
    }
    fn item_slot_by_guid(&self, _account_id: u64, item_guid: u64) -> Option<u8> {
        self.item_slots
            .iter()
            .find(|(g, _)| *g == item_guid)
            .map(|&(_, s)| s)
    }
    fn disenchant_item(&self, _account_id: u64, _self_guid: u64, slot: u8) -> Result<()> {
        self.disenchanted.lock().unwrap().push(slot);
        Ok(())
    }
    fn enchant_item_on_slot(
        &self,
        _account_id: u64,
        _self_guid: u64,
        slot: u8,
        enchant_id: u32,
    ) -> Result<()> {
        self.enchanted.lock().unwrap().push((slot, enchant_id));
        Ok(())
    }
    fn talent_grant_spell(&self, _talent_id: u32) -> u32 {
        self.talent_grant
    }
    fn spell_is_ground_area(&self, _spell_id: u32) -> bool {
        false
    }
    fn spell_is_fishing(&self, spell_id: u32) -> bool {
        self.fishing_spells.contains(&spell_id)
    }
    fn fish(&self, _account_id: u64, _self_guid: u64) -> Result<()> {
        self.fish_casts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        match &self.trade_error {
            Some(e) => Err(anyhow!("{e}")),
            None => Ok(()),
        }
    }
    fn spell_is_open_lock(&self, spell_id: u32) -> bool {
        self.open_lock_spells.contains(&spell_id)
    }
    fn pick_lock(&self, _account_id: u64, _self_guid: u64, go_guid: u64) -> Result<()> {
        self.pick_lock_casts.lock().unwrap().push(go_guid);
        match &self.trade_error {
            Some(e) => Err(anyhow!("{e}")),
            None => Ok(()),
        }
    }
    fn set_faction_at_war(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _reputation_index: u32,
        _at_war: bool,
    ) -> Result<()> {
        Ok(())
    }
    fn set_action_button(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _button: u8,
        _action: u32,
        _action_type: u8,
    ) -> Result<()> {
        Ok(())
    }
    fn talent_pane_sync(&self, _character_guid: u64, _talent_id: u32) -> (u32, u32, u32) {
        self.talent_pane
    }
    fn talent_points_spent(&self, _character_guid: u64) -> u32 {
        0 // login stays byte-identical in every existing harness test
    }
    fn spell_modifiers(&self, _character_guid: u64) -> Vec<(u32, u8, i32, bool)> {
        Vec::new() // no modifier packets in the harness (login stays byte-identical)
    }
    fn learn_talent(&self, _account_id: u64, _self_guid: u64, _talent_id: u32) -> Result<()> {
        match &self.trade_error {
            Some(e) => Err(anyhow!("{e}")),
            None => Ok(()),
        }
    }
    fn push_quest(&self, account_id: u64, _self_guid: u64, quest_id: u32) -> Result<()> {
        if let Some(e) = &self.push_quest_error {
            return Err(anyhow!("{e}"));
        }
        self.pushed_quests
            .lock()
            .unwrap()
            .push((account_id, quest_id));
        Ok(())
    }
    fn bind_home(&self, _account_id: u64, _self_guid: u64) -> Result<()> {
        self.home_bound
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    fn npc_is_innkeeper(&self, _guid: u64) -> Result<bool> {
        Ok(self.innkeeper)
    }
    fn npc_gossip_text_id(&self, _npc_guid: u64) -> u32 {
        1 // generic fallback for tests
    }
    fn npc_text_for_id(&self, _text_id: u32) -> Option<codec::NpcTextView> {
        self.npc_text_view.clone()
    }
    fn gossip_options(&self, _npc_guid: u64) -> Result<Vec<codec::GossipOptionView>> {
        Ok(self.gossip_opts.clone())
    }
    fn quest_status(&self, _guid: u64, quest_id: u32) -> (bool, bool) {
        match self
            .quest_log
            .lock()
            .unwrap()
            .iter()
            .find(|(id, _)| *id == quest_id)
        {
            Some((_, rewarded)) => (true, *rewarded),
            None => (false, false),
        }
    }
    fn reset_talents(&self, account_id: u64, self_guid: u64, trainer_guid: u64) -> Result<()> {
        if let Some(e) = &self.reset_talents_error {
            return Err(anyhow!("{e}"));
        }
        self.reset_talents_calls
            .lock()
            .unwrap()
            .push((account_id, self_guid, trainer_guid));
        Ok(())
    }
    fn auto_bank_item(&self, _account_id: u64, _self_guid: u64, slot: u8) -> Result<()> {
        if let Some(e) = &self.trade_error {
            return Err(anyhow!("{e}"));
        }
        self.auto_banked_items.lock().unwrap().push(slot);
        Ok(())
    }
    fn buy_bank_slot(&self, _account_id: u64, _self_guid: u64, banker_guid: u64) -> Result<()> {
        if let Some(e) = &self.trade_error {
            return Err(anyhow!("{e}"));
        }
        self.bought_bank_slots.lock().unwrap().push(banker_guid);
        Ok(())
    }
    fn quest_giver_evals(
        &self,
        _giver_guid: u64,
        _player_guid: u64,
    ) -> Result<Vec<codec::GiverQuestEval>> {
        Ok(self.quest_evals.clone())
    }
    fn quest_detail(&self, quest_id: u32) -> Result<Option<codec::QuestDetailView>> {
        Ok(self
            .quest_details
            .iter()
            .find(|d| d.quest_id == quest_id)
            .cloned())
    }
    fn accept_quest(
        &self,
        account_id: u64,
        _self_guid: u64,
        giver_guid: u64,
        quest_id: u32,
    ) -> Result<()> {
        if let Some(e) = &self.trade_error {
            return Err(anyhow!("{e}"));
        }
        self.accepted
            .lock()
            .unwrap()
            .push((account_id, giver_guid, quest_id));
        Ok(())
    }
    fn turn_in_quest(
        &self,
        account_id: u64,
        _self_guid: u64,
        giver_guid: u64,
        quest_id: u32,
        reward_index: u32,
    ) -> Result<()> {
        if let Some(e) = &self.trade_error {
            return Err(anyhow!("{e}"));
        }
        self.turned_in
            .lock()
            .unwrap()
            .push((account_id, giver_guid, quest_id, reward_index));
        Ok(())
    }
    fn player_quest_log(&self, _player_guid: u64) -> Result<Vec<codec::update_mask::QuestLogSlot>> {
        Ok(self.quest_log_slots.clone())
    }
    fn player_learned_spells(&self, _player_guid: u64) -> Result<Vec<u32>> {
        Ok(Vec::new())
    }
    fn player_reputations(&self, _player_guid: u64) -> Result<Vec<(i32, i32, bool)>> {
        Ok(self.reputations.clone())
    }
    fn player_actions(&self, _player_guid: u64) -> Result<Vec<(u8, u32, u8)>> {
        Ok(self.player_actions.clone())
    }
    fn buyback_ring(&self, _player_guid: u64) -> Vec<(u32, u32, u32)> {
        Vec::new()
    }
    fn resolve_learn_target(&self, spell_id: u32) -> u32 {
        spell_id // mock: self-contained ranks (no wrapper table in the mock store)
    }
    fn entity_in_world(&self, guid: u64) -> bool {
        self.entity_presence_checks
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Some(present) = &self.entity_presence {
            return present.load(std::sync::atomic::Ordering::SeqCst);
        }
        // `live_guids` is the per-guid answer the realm-wide party frame needs ("is this member
        // live on THIS shard"). Empty by default, so the single flag above is still the answer
        // every test written before realm-wide party routing set.
        self.entity_in_world || self.live_guids.contains(&guid)
    }
    fn abandon_quest(&self, account_id: u64, _self_guid: u64, quest_id: u32) -> Result<()> {
        if let Some(e) = &self.trade_error {
            return Err(anyhow!("{e}"));
        }
        self.abandoned.lock().unwrap().push((account_id, quest_id));
        Ok(())
    }
    fn set_target(&self, _account_id: u64, _self_guid: u64, target_guid: u64) -> Result<()> {
        self.rec("set_target");
        if let Some(e) = &self.set_target_error {
            return Err(anyhow!("{e}"));
        }
        self.selected_targets.lock().unwrap().push(target_guid);
        Ok(())
    }
    fn inspect(&self, _account_id: u64, _self_guid: u64, target_guid: u64) -> Result<()> {
        if let Some(e) = &self.trade_error {
            return Err(anyhow!("{e}"));
        }
        // Mirrors the module gate's own-map/in-range/friendly checks with a fixed stub: any nonzero
        // guid "passes" (in range + friendly) so a test can drive both the ack and the ignore path via
        // `trade_error`; a 0 guid stands in for "no such target".
        if target_guid == 0 {
            return Err(anyhow!("no such inspect target"));
        }
        Ok(())
    }
    fn start_attack(&self, _account_id: u64, _self_guid: u64, _target_guid: u64) -> Result<()> {
        self.rec("start_attack");
        match &self.start_attack_error {
            Some(e) => Err(anyhow!("{e}")),
            None => Ok(()),
        }
    }
    fn pet_command(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _data: u32,
        _target_guid: u64,
    ) -> Result<()> {
        Ok(())
    }
    fn start_ranged_attack(
        &self,
        _account_id: u64,
        _self_guid: u64,
        target_guid: u64,
        spell_id: u32,
    ) -> Result<()> {
        if let Some(e) = &self.start_ranged_attack_error {
            return Err(anyhow!("{e}"));
        }
        self.ranged_attacks
            .lock()
            .unwrap()
            .push((target_guid, spell_id));
        Ok(())
    }
    fn stop_attack(&self, _account_id: u64, _self_guid: u64) -> Result<()> {
        Ok(())
    }
    fn set_sheathed(&self, _account_id: u64, self_guid: u64, state: u8) -> Result<()> {
        self.sheathed.lock().unwrap().push((self_guid, state));
        Ok(())
    }
    fn cast_spell(
        &self,
        _account_id: u64,
        _self_guid: u64,
        spell_id: u32,
        target_guid: u64,
    ) -> Result<()> {
        if let Some(e) = &self.cast_spell_error {
            return Err(anyhow!("{e}"));
        }
        self.casts.lock().unwrap().push((spell_id, target_guid));
        Ok(())
    }
    fn cast_spell_at(
        &self,
        _account_id: u64,
        _self_guid: u64,
        spell_id: u32,
        target_guid: u64,
        x: f32,
        y: f32,
        z: f32,
    ) -> Result<()> {
        if let Some(e) = &self.cast_spell_error {
            return Err(anyhow!("{e}"));
        }
        self.ground_casts
            .lock()
            .unwrap()
            .push((spell_id, target_guid, x, y, z));
        Ok(())
    }
    fn cancel_aura(&self, _account_id: u64, _self_guid: u64, spell_id: u32) -> Result<()> {
        self.cancelled_auras.lock().unwrap().push(spell_id);
        Ok(())
    }
    fn cancel_cast(&self, _account_id: u64, self_guid: u64) -> Result<()> {
        self.cancelled_casts.lock().unwrap().push(self_guid);
        Ok(())
    }
    fn spell_cast_time(&self, _spell_id: u32) -> Option<u32> {
        self.cast_time_ms
    }
    fn spell_queues_next_swing(&self, _spell_id: u32) -> bool {
        self.queues_next_swing
    }
    fn spell_is_ranged_auto_repeat(&self, spell_id: u32) -> bool {
        // Mirrors the real RANGED_AUTO_REPEAT cast_flags bit for the two vanilla auto-repeat abilities.
        matches!(spell_id, 75 | 5019)
    }
    fn entity_max_health(&self, _guid: u64) -> u32 {
        100
    }
    fn join_channel(&self, _account_id: u64, _self_guid: u64, channel: String) -> Result<()> {
        self.channel_joins.lock().unwrap().push(channel);
        Ok(())
    }
    fn leave_channel(&self, _account_id: u64, _self_guid: u64, _channel: String) -> Result<()> {
        Ok(())
    }
    fn send_channel_message(
        &self,
        _account_id: u64,
        _self_guid: u64,
        channel: String,
        message: String,
    ) -> Result<()> {
        self.channel_messages
            .lock()
            .unwrap()
            .push((channel, message));
        Ok(())
    }
    fn superseded_old_rank(&self, _new_spell: u32, _player_guid: u64) -> Option<u32> {
        None
    }
    fn enchant_route(&self, _spell_id: u32) -> Option<super::EnchantRoute> {
        self.enchant_route
    }
    fn send_chat(
        &self,
        _account_id: u64,
        _self_guid: u64,
        chat_type: u8,
        language: u8,
        message: String,
    ) -> Result<()> {
        // Recorded per SHARD like every other player-scoped call, so the partition rule (say/
        // yell stay shard-local and range-scoped) is assertable rather than merely stated.
        self.rec("send_chat");
        self.chats
            .lock()
            .unwrap()
            .push((chat_type, language, message));
        Ok(())
    }
    fn send_emote(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _text_emote: u32,
        _emote_anim: u32,
        _target_guid: u64,
    ) -> Result<()> {
        self.rec("send_emote");
        Ok(())
    }
    fn send_roll(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _min_roll: u32,
        _max_roll: u32,
    ) -> Result<()> {
        Ok(())
    }
    fn send_whisper(
        &self,
        _account_id: u64,
        _self_guid: u64,
        target_player: String,
        message: String,
    ) -> Result<()> {
        // Recorded per SHARD, so a test can tell the pre-realm-core path (the
        // player-facing reducer on the player's own database, with the TYPED NAME still unresolved)
        // from the realm-core one (`realm_whispers`, by guid).
        self.rec("send_whisper");
        self.whispers.lock().unwrap().push((target_player, message));
        match &self.whisper_error {
            Some(e) => Err(anyhow!("{e}")),
            None => Ok(()),
        }
    }
    fn party_chat(&self, _account_id: u64, _self_guid: u64, message: String) -> Result<()> {
        match &self.party_chat_error {
            Some(e) => Err(anyhow!("{e}")),
            None => {
                self.party_chats.lock().unwrap().push(message);
                Ok(())
            }
        }
    }
    fn gm_command(&self, _account_id: u64, _self_guid: u64, text: String) -> Result<()> {
        match &self.gm_command_error {
            Some(e) => Err(anyhow!("{e}")),
            None => {
                self.gm_commands.lock().unwrap().push(text);
                Ok(())
            }
        }
    }
    fn loot_target_money(&self, _target_guid: u64) -> Result<u32> {
        Ok(self.corpse_money)
    }
    fn loot_money(&self, _account_id: u64, _self_guid: u64, target_guid: u64) -> Result<()> {
        self.money_looted.lock().unwrap().push(target_guid);
        Ok(())
    }
    fn take_loot(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _corpse_guid: u64,
        _loot_slot: u8,
    ) -> Result<()> {
        Ok(())
    }
    fn repop(&self, _account_id: u64, self_guid: u64) -> Result<()> {
        self.repopped.lock().unwrap().push(self_guid);
        Ok(())
    }
    fn claim_session(&self, _account_id: u64) -> u64 {
        1
    }
    fn release_session(&self, _account_id: u64, _epoch: u64) -> bool {
        // Default (false) = this session still owns the entity; `stale_session` simulates a newer
        // login having superseded it (the session-epoch arbitration), so teardown must skip `logout`.
        !self.stale_session
    }
    fn reclaim_corpse(&self, _account_id: u64, self_guid: u64, corpse_guid: u64) -> Result<()> {
        self.reclaimed_corpses
            .lock()
            .unwrap()
            .push((self_guid, corpse_guid));
        Ok(())
    }
    fn resurrect_response(&self, _account_id: u64, self_guid: u64, accept: bool) -> Result<()> {
        self.resurrect_responses
            .lock()
            .unwrap()
            .push((self_guid, accept));
        Ok(())
    }
    fn spirit_healer_res(&self, _account_id: u64, self_guid: u64, healer_guid: u64) -> Result<()> {
        self.spirit_healer_calls
            .lock()
            .unwrap()
            .push((self_guid, healer_guid));
        Ok(())
    }
    fn corpse_location(&self, _owner_guid: u64) -> Result<Option<(u32, f32, f32, f32)>> {
        Ok(None)
    }
    fn player_combat_until_ms(&self, _player_guid: u64) -> u64 {
        self.combat_until_ms
    }
    fn online_players(&self) -> Result<Vec<codec::WhoPlayerView>> {
        // Test store: return the seeded characters as "online" so CMSG_WHO tests can assert a response.
        Ok(self
            .characters
            .iter()
            .map(|c| codec::WhoPlayerView {
                name: c.name.clone(),
                level: c.level,
                class: c.class,
                race: c.race,
                zone_id: c.zone_id,
            })
            .collect())
    }
    fn contact_lists(&self, self_guid: u64) -> Result<(Vec<codec::FriendView>, Vec<u64>)> {
        if let Some(e) = &self.contact_lists_error {
            return Err(anyhow!("{e}"));
        }
        let contacts = self.contacts.lock().unwrap();
        let mut friends = Vec::new();
        let mut ignored = Vec::new();
        for &(owner, target, is_ignore) in contacts.iter() {
            if owner != self_guid {
                continue;
            }
            if is_ignore {
                ignored.push(target);
            } else {
                // Test store: every seeded character is "online" (mirrors online_players above).
                let (online, level, class, zone_id) = self
                    .characters
                    .iter()
                    .find(|c| c.guid == target)
                    .map(|c| (true, c.level, c.class, c.zone_id))
                    .unwrap_or((false, 0, 0, 0));
                friends.push(codec::FriendView {
                    guid: target,
                    online,
                    level,
                    class,
                    zone_id,
                });
            }
        }
        Ok((friends, ignored))
    }
    fn character_guid_by_name(&self, name: &str) -> Result<Option<u64>> {
        Ok(self
            .characters
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
            .map(|c| c.guid))
    }
    fn character_presence(&self, guid: u64) -> Result<Option<(bool, u8, u8, u32)>> {
        Ok(self
            .characters
            .iter()
            .find(|c| c.guid == guid)
            // `offline_guids` drives the invite gate's "player not online" arm. Empty by
            // default, so a seeded character is online exactly as it always was.
            .map(|c| {
                (
                    !self.offline_guids.contains(&guid),
                    c.level,
                    c.class,
                    c.zone_id,
                )
            }))
    }
    // Group: a minimal in-memory party — enough for the dispatch tests to drive
    // invite-result mapping and the GROUP_LIST build without a live module.
    //
    // Each of these records the SHARD it ran on (`rec`), so a test can tell the
    // single-database path (the op lands on the player's own shard, here) apart from the realm-core
    // one (it lands in `FakeParty::ops` and never reaches these at all).
    fn group_invite(&self, _account_id: u64, _self_guid: u64, target_guid: u64) -> Result<()> {
        self.rec("group_invite");
        if let Some(e) = &self.trade_error {
            return Err(anyhow!("{e}"));
        }
        self.group_invites.lock().unwrap().push(target_guid);
        Ok(())
    }
    fn group_accept(&self, _account_id: u64, _self_guid: u64) -> Result<()> {
        self.rec("group_accept");
        Ok(())
    }
    // Trade (#120): pure recorders — the module owns every gate, so the fake just proves which
    // verb the dispatch chose and which args survived the wire.
    fn initiate_trade(&self, _account_id: u64, self_guid: u64, target_guid: u64) -> Result<()> {
        self.rec("initiate_trade");
        self.initiated_trades
            .lock()
            .unwrap()
            .push((self_guid, target_guid));
        Ok(())
    }
    fn begin_trade(&self, _account_id: u64, self_guid: u64) -> Result<()> {
        self.rec("begin_trade");
        self.begun_trades.lock().unwrap().push(self_guid);
        Ok(())
    }
    fn cancel_trade(&self, _account_id: u64, self_guid: u64) -> Result<()> {
        self.rec("cancel_trade");
        self.cancelled_trades.lock().unwrap().push(self_guid);
        Ok(())
    }
    fn set_trade_item(
        &self,
        _account_id: u64,
        self_guid: u64,
        trade_slot: u8,
        inv_slot: u8,
    ) -> Result<()> {
        self.rec("set_trade_item");
        self.set_trade_items
            .lock()
            .unwrap()
            .push((self_guid, trade_slot, inv_slot));
        Ok(())
    }
    fn clear_trade_item(&self, _account_id: u64, self_guid: u64, trade_slot: u8) -> Result<()> {
        self.rec("clear_trade_item");
        self.cleared_trade_items
            .lock()
            .unwrap()
            .push((self_guid, trade_slot));
        Ok(())
    }
    fn set_trade_gold(&self, _account_id: u64, self_guid: u64, copper: u32) -> Result<()> {
        self.rec("set_trade_gold");
        self.set_trade_golds
            .lock()
            .unwrap()
            .push((self_guid, copper));
        Ok(())
    }
    fn accept_trade(&self, _account_id: u64, self_guid: u64) -> Result<()> {
        self.rec("accept_trade");
        self.accepted_trades.lock().unwrap().push(self_guid);
        Ok(())
    }
    fn unaccept_trade(&self, _account_id: u64, self_guid: u64) -> Result<()> {
        self.rec("unaccept_trade");
        self.unaccepted_trades.lock().unwrap().push(self_guid);
        Ok(())
    }
    fn busy_trade(&self, _account_id: u64, self_guid: u64) -> Result<()> {
        self.rec("busy_trade");
        self.busy_trades.lock().unwrap().push(self_guid);
        Ok(())
    }
    fn ignore_trade(&self, _account_id: u64, self_guid: u64) -> Result<()> {
        self.rec("ignore_trade");
        self.ignore_trades.lock().unwrap().push(self_guid);
        Ok(())
    }
    fn group_decline(&self, _account_id: u64, _self_guid: u64) -> Result<()> {
        self.rec("group_decline");
        Ok(())
    }
    fn group_leave(&self, _account_id: u64, _self_guid: u64) -> Result<()> {
        self.rec("group_leave");
        Ok(())
    }
    fn group_loot_method(
        &self,
        _account_id: u64,
        _self_guid: u64,
        loot_setting: u8,
        master_guid: u64,
        loot_threshold: u8,
    ) -> Result<()> {
        self.rec("group_loot_method");
        if let Some(e) = &self.trade_error {
            return Err(anyhow!("{e}"));
        }
        self.group_loot_methods
            .lock()
            .unwrap()
            .push((loot_setting, master_guid, loot_threshold));
        Ok(())
    }

    // --- The realm-core plane (party/group routing) ---

    fn realm_store(&self) -> Option<std::sync::Arc<dyn WorldStore>> {
        self.realm
            .clone()
            .map(|r| r as std::sync::Arc<dyn WorldStore>)
    }

    fn world_stores(&self) -> Vec<std::sync::Arc<dyn WorldStore>> {
        self.peers
            .lock()
            .unwrap()
            .iter()
            .map(|p| p.clone() as std::sync::Arc<dyn WorldStore>)
            .collect()
    }

    /// The module's `realm_group_op`, modelled: the rules the ROUTING depends on, applied to the
    /// authority this handle owns.
    fn realm_group_op(
        &self,
        op: u8,
        actor_guid: u64,
        target_guid: u64,
        arg_a: u8,
        arg_b: u8,
    ) -> Result<()> {
        use lyracore_shared::group::{err as group_err, event_kind as kind, realm_op};
        self.rec("realm_group_op");
        let mut p = self.party.lock().unwrap();
        p.ops.push((op, actor_guid, target_guid, arg_a, arg_b));
        match op {
            realm_op::INVITE => {
                if p.group_of(target_guid).is_some() {
                    return Err(anyhow!("{}", group_err::ALREADY_IN_GROUP));
                }
                p.invites.retain(|(t, _)| *t != target_guid);
                p.invites.push((target_guid, actor_guid));
                p.events.push((target_guid, kind::INVITE));
            }
            realm_op::ACCEPT => {
                if let Some(e) = &self.party_accept_error {
                    return Err(anyhow!("{e}"));
                }
                let inviter = p
                    .invites
                    .iter()
                    .find(|(t, _)| *t == actor_guid)
                    .map(|(_, i)| *i)
                    .ok_or_else(|| anyhow!("no pending invite"))?;
                p.invites.retain(|(t, _)| *t != actor_guid);
                let group_id = match p.group_of(inviter) {
                    Some(g) => g,
                    None => {
                        p.next_group_id += 1;
                        let g = p.next_group_id;
                        // The vanilla defaults a freshly-formed party gets: GROUP loot, Uncommon.
                        p.groups.push((g, inviter, 3, 2, 0));
                        p.members.push((g, inviter));
                        g
                    }
                };
                p.members.push((group_id, actor_guid));
                p.push_list(group_id);
            }
            realm_op::DECLINE => {
                let inviter = p
                    .invites
                    .iter()
                    .find(|(t, _)| *t == actor_guid)
                    .map(|(_, i)| *i)
                    .ok_or_else(|| anyhow!("no pending invite"))?;
                p.invites.retain(|(t, _)| *t != actor_guid);
                p.events.push((inviter, kind::DECLINE));
            }
            realm_op::LEAVE => {
                if p.group_of(actor_guid).is_none() {
                    return Err(anyhow!("{}", group_err::NOT_IN_GROUP));
                }
                p.remove_member(actor_guid);
            }
            realm_op::UNINVITE => {
                let group_id = p
                    .group_of(actor_guid)
                    .ok_or_else(|| anyhow!("{}", group_err::NOT_IN_GROUP))?;
                if p.groups.iter().find(|(g, ..)| *g == group_id).map(|e| e.1) != Some(actor_guid) {
                    return Err(anyhow!("{}", group_err::NOT_LEADER));
                }
                if p.group_of(target_guid) != Some(group_id) {
                    return Err(anyhow!("{}", group_err::TARGET_NOT_IN_GROUP));
                }
                p.remove_member(target_guid);
            }
            realm_op::LOOT_METHOD => {
                let group_id = p
                    .group_of(actor_guid)
                    .ok_or_else(|| anyhow!("{}", group_err::NOT_IN_GROUP))?;
                if let Some(entry) = p.groups.iter_mut().find(|(g, ..)| *g == group_id) {
                    if entry.1 != actor_guid {
                        return Err(anyhow!("{}", group_err::NOT_LEADER));
                    }
                    entry.2 = arg_a;
                    entry.3 = arg_b;
                    entry.4 = target_guid;
                }
                p.push_list(group_id);
            }
            other => return Err(anyhow!("unknown realm group op {other}")),
        }
        Ok(())
    }

    fn group_roster(&self, character_guid: u64) -> Result<Option<super::party::GroupRoster>> {
        if self.is_realm {
            let p = self.party.lock().unwrap();
            return Ok(p.group_of(character_guid).and_then(|g| p.roster(g)));
        }
        Ok(self
            .mirror
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.members.contains(&character_guid))
            .cloned())
    }

    fn group_roster_by_id(&self, group_id: u64) -> Result<Option<super::party::GroupRoster>> {
        if self.is_realm {
            return Ok(self.party.lock().unwrap().roster(group_id));
        }
        Ok(self
            .mirror
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.group_id == group_id)
            .cloned())
    }

    fn sync_group_mirror(&self, roster: &super::party::GroupRoster) -> Result<()> {
        self.rec("sync_group_mirror");
        if let Some(e) = &self.mirror_error {
            return Err(anyhow!("{e}"));
        }
        let mut mirror = self.mirror.lock().unwrap();
        mirror.retain(|r| r.group_id != roster.group_id);
        // An empty roster is the disband tombstone — the shard forgets the party rather than
        // keeping an empty one, which is what the module's own `sync_group_mirror` does.
        if !roster.members.is_empty() {
            mirror.push(roster.clone());
        }
        Ok(())
    }

    /// The module's `realm_whisper`, modelled: it RECORDS the tuple it was told to deliver before
    /// judging anything, because what these tests pin is what the GATEWAY claimed (the sender guid
    /// especially — it is the whole authorization on this plane).
    fn realm_whisper(
        &self,
        sender_guid: u64,
        target_guid: u64,
        message: String,
        sender_is_ignored: bool,
    ) -> Result<()> {
        self.rec("realm_whisper");
        self.realm_whispers.lock().unwrap().push((
            sender_guid,
            target_guid,
            message,
            sender_is_ignored,
        ));
        if let Some(e) = &self.realm_whisper_error {
            return Err(anyhow!("{e}"));
        }
        Ok(())
    }
    fn loot_roll(
        &self,
        _account_id: u64,
        _self_guid: u64,
        corpse_guid: u64,
        loot_slot: u32,
        vote: u8,
    ) -> Result<()> {
        if let Some(e) = &self.trade_error {
            return Err(anyhow!("{e}"));
        }
        self.loot_rolls
            .lock()
            .unwrap()
            .push((corpse_guid, loot_slot, vote));
        Ok(())
    }
    fn loot_master_give(
        &self,
        _account_id: u64,
        _self_guid: u64,
        corpse_guid: u64,
        loot_slot: u8,
        target_guid: u64,
    ) -> Result<()> {
        if let Some(e) = &self.trade_error {
            return Err(anyhow!("{e}"));
        }
        self.loot_master_gives
            .lock()
            .unwrap()
            .push((corpse_guid, loot_slot, target_guid));
        Ok(())
    }

    // --- Realm-wide loot rolls ---

    #[allow(clippy::too_many_arguments)]
    fn realm_loot_op(
        &self,
        op: u8,
        corpse_guid: u64,
        slot: u8,
        item_entry: u32,
        actor_guid: u64,
        vote: u8,
        deadline_micros: i64,
        recipients: Vec<u64>,
    ) -> Result<()> {
        self.rec("realm_loot_op");
        self.realm_loot_ops.lock().unwrap().push((
            op,
            corpse_guid,
            slot,
            item_entry,
            actor_guid,
            vote,
            deadline_micros,
            recipients,
        ));
        if let Some(e) = &self.realm_loot_op_error {
            return Err(anyhow!("{e}"));
        }
        Ok(())
    }

    fn pending_local_rolls(&self) -> Result<Vec<super::loot::PendingLootRoll>> {
        Ok(self.pending_rolls.lock().unwrap().clone())
    }

    fn settle_loot_roll(&self, corpse_guid: u64, slot: u8, winner_guid: u64) -> Result<()> {
        self.rec("settle_loot_roll");
        if let Some(e) = &self.settle_loot_roll_error {
            return Err(anyhow!("{e}"));
        }
        self.settled_rolls
            .lock()
            .unwrap()
            .push((corpse_guid, slot, winner_guid));
        Ok(())
    }

    fn clear_promoted_loot_roll(&self, roll_id: u64) -> Result<()> {
        self.rec("clear_promoted_loot_roll");
        self.cleared_rolls.lock().unwrap().push(roll_id);
        Ok(())
    }

    fn loot_won_since(&self, after_id: u64) -> Result<(u64, Vec<(u64, u8, u64)>)> {
        let events = self.won_events.lock().unwrap();
        let watermark = events.len() as u64;
        let wins = events
            .iter()
            .enumerate()
            .filter(|(i, _)| (*i as u64 + 1) > after_id)
            .map(|(_, w)| *w)
            .collect();
        Ok((watermark, wins))
    }

    fn group_uninvite(&self, _account_id: u64, _self_guid: u64, _target_guid: u64) -> Result<()> {
        self.rec("group_uninvite");
        Ok(())
    }
    fn gossip_select(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _npc_guid: u64,
        option_id: u32,
        option_row_id: u32,
    ) -> Result<()> {
        self.gossip_selects
            .lock()
            .unwrap()
            .push((option_id, option_row_id));
        Ok(())
    }
    fn add_friend(&self, _account_id: u64, _self_guid: u64, target_guid: u64) -> Result<()> {
        if let Some(e) = &self.trade_error {
            return Err(anyhow!("{e}"));
        }
        let owner = self.login_entity.as_ref().map(|e| e.guid).unwrap_or(0);
        self.contacts
            .lock()
            .unwrap()
            .push((owner, target_guid, false));
        Ok(())
    }
    fn del_friend(&self, _account_id: u64, _self_guid: u64, target_guid: u64) -> Result<()> {
        let owner = self.login_entity.as_ref().map(|e| e.guid).unwrap_or(0);
        let mut contacts = self.contacts.lock().unwrap();
        let before = contacts.len();
        contacts.retain(|&(o, t, ig)| !(o == owner && t == target_guid && !ig));
        if contacts.len() == before {
            return Err(anyhow!("not on that list"));
        }
        Ok(())
    }
    fn add_ignore(&self, _account_id: u64, _self_guid: u64, target_guid: u64) -> Result<()> {
        if let Some(e) = &self.trade_error {
            return Err(anyhow!("{e}"));
        }
        let owner = self.login_entity.as_ref().map(|e| e.guid).unwrap_or(0);
        self.contacts
            .lock()
            .unwrap()
            .push((owner, target_guid, true));
        Ok(())
    }
    fn del_ignore(&self, _account_id: u64, _self_guid: u64, target_guid: u64) -> Result<()> {
        let owner = self.login_entity.as_ref().map(|e| e.guid).unwrap_or(0);
        let mut contacts = self.contacts.lock().unwrap();
        let before = contacts.len();
        contacts.retain(|&(o, t, ig)| !(o == owner && t == target_guid && ig));
        if contacts.len() == before {
            return Err(anyhow!("not on that list"));
        }
        Ok(())
    }
}

impl VendorActionStore for InMemoryStore {
    fn vendor_stock(&self, _vendor_guid: u64) -> Result<Vec<codec::VendorItemView>> {
        Ok(self.vendor_stock.clone())
    }

    fn vendor_refuses_interaction(&self, _vendor_guid: u64, _player_guid: u64) -> Result<bool> {
        Ok(self.npc_refuses)
    }

    fn vendor_item_slot(&self, item_guid: u64) -> Option<u8> {
        self.item_slots
            .iter()
            .find(|(g, _)| *g == item_guid)
            .map(|&(_, s)| s)
    }

    fn vendor_repair(
        &self,
        _account_id: u64,
        _self_guid: u64,
        npc_guid: u64,
        slot: u8,
    ) -> Result<()> {
        if let Some(e) = &self.trade_error {
            return Err(anyhow!("{e}"));
        }
        self.repaired_items.lock().unwrap().push((npc_guid, slot));
        Ok(())
    }
}

impl ItemActionStore for InMemoryStore {
    fn equip_item(&self, _account_id: u64, _self_guid: u64, _from_slot: u8) -> Result<()> {
        match &self.trade_error {
            Some(e) => Err(anyhow!("{e}")),
            None => Ok(()),
        }
    }

    fn unequip_item(&self, _account_id: u64, _self_guid: u64, from_slot: u8) -> Result<()> {
        if let Some(e) = &self.trade_error {
            return Err(anyhow!("{e}"));
        }
        self.unequipped_slots.lock().unwrap().push(from_slot);
        Ok(())
    }

    fn move_item(
        &self,
        _account_id: u64,
        _self_guid: u64,
        from_slot: u8,
        to_slot: u8,
    ) -> Result<()> {
        if let Some(e) = &self.trade_error {
            return Err(anyhow!("{e}"));
        }
        self.moved_items.lock().unwrap().push((from_slot, to_slot));
        Ok(())
    }

    fn use_item(&self, _account_id: u64, _self_guid: u64, slot: u8) -> Result<()> {
        self.used_items.lock().unwrap().push(slot);
        match &self.trade_error {
            Some(e) => Err(anyhow!("{e}")),
            None => Ok(()),
        }
    }

    fn item_start_quest(&self, _owner_guid: u64, _slot: u8) -> Option<(u64, u32)> {
        self.item_start_quest_fixture
    }

    fn item_quest_detail(&self, quest_id: u32) -> Result<Option<codec::QuestDetailView>> {
        Ok(self
            .quest_details
            .iter()
            .find(|d| d.quest_id == quest_id)
            .cloned())
    }
}

fn ns(s: &str) -> NormalizedString {
    NormalizedString::new(s).unwrap()
}

/// Drive the shared prefix of every world handshake: read the plaintext `SMSG_AUTH_CHALLENGE`,
/// derive the client-side proof + cipher pair for `key` with `wow_srp`, and send
/// `CMSG_AUTH_SESSION`. Lower-level than [`client_handshake`] — it does not read whatever comes
/// back, so a call site can assert on that itself (an `AuthOk`, an `AuthWaitQueue`, a plaintext
/// rejection...). Returns the cipher pair `into_client_header_crypto` derived, split; a `key` that
/// does not match what the server holds still produces a (mismatched, useless) pair here — the
/// send happens regardless — so a rejection-path call site is free to bind them as `_`.
fn drive_auth<S: Read + Write>(
    client: &mut S,
    username: &str,
    key: [u8; 40],
) -> (EncrypterHalf, DecrypterHalf) {
    let server_seed = match ServerOpcodeMessage::read_unencrypted(&mut *client).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_CHALLENGE(c) => c.server_seed,
        other => panic!("expected SMSG_AUTH_CHALLENGE, got {other}"),
    };
    let client_seed = ProofSeed::new();
    let client_seed_value = client_seed.seed();
    let (client_proof, crypto) =
        client_seed.into_client_header_crypto(&ns(username), key, server_seed);
    let (enc, dec) = crypto.split();

    auth_session(username, client_seed_value, client_proof)
        .write_unencrypted_client(&mut *client)
        .unwrap();

    (enc, dec)
}

/// Drive the client side of the world handshake against a server running `run_world_session`
/// (or `world_handshake`): read the plaintext challenge, send `CMSG_AUTH_SESSION` with a
/// valid proof, read the encrypted AUTH_OK. Returns the client's cipher halves for the
/// post-handshake encrypted traffic.
fn client_handshake<S: Read + Write>(
    client: &mut S,
    username: &str,
    key: [u8; 40],
) -> (EncrypterHalf, DecrypterHalf) {
    let (enc, mut dec) = drive_auth(client, username, key);

    match ServerOpcodeMessage::read_encrypted(&mut *client, &mut dec).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_RESPONSE(r) => {
            assert!(matches!(*r, SMSG_AUTH_RESPONSE::AuthOk { .. }));
        }
        other => panic!("expected encrypted SMSG_AUTH_RESPONSE, got {other}"),
    }
    (enc, dec)
}

fn auth_session(username: &str, client_seed: u32, client_proof: [u8; 20]) -> CMSG_AUTH_SESSION {
    CMSG_AUTH_SESSION {
        build: 5875,
        server_id: 1,
        username: username.to_string(),
        client_seed,
        client_proof,
        addon_info: vec![],
    }
}

/// A store for a logged-in TESTER (account `account_id`), with no character/scenario state beyond
/// the session itself — the shape every handshake/login/logout/movement test overlays with its own
/// fields via `..`. `quest_store()` is the sibling for tests that also need a login entity.
fn tester_store(account_id: u64) -> InMemoryStore {
    InMemoryStore {
        entity_in_world: true,
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id,
            session_key: K,
        }),
        ..Default::default()
    }
}

#[test]
fn handshake_succeeds_and_traffic_is_encrypted_both_ways() {
    let store = std::sync::Arc::new(tester_store(42));

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        let mut s = server_end;
        let (mut conn, _encrypt) = world_handshake(&mut s, server_store.as_ref())
            .unwrap()
            .expect("handshake should succeed");
        assert_eq!(conn.account_id, 42);
        // Prove the inbound cipher works: read one encrypted client message.
        match ClientOpcodeMessage::read_encrypted(&mut s, &mut conn.decrypt).unwrap() {
            ClientOpcodeMessage::CMSG_CHAR_ENUM => {}
            other => panic!("expected encrypted CMSG_CHAR_ENUM, got {other}"),
        }
    });

    // --- client: drive the shared challenge→proof→AUTH_SESSION prefix (`drive_auth`) ---
    let (mut c_enc, mut c_dec) = drive_auth(&mut client, "TESTER", K);

    // --- client: the AUTH_OK response is the first ENCRYPTED packet ---
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_RESPONSE(r) => {
            assert!(matches!(*r, SMSG_AUTH_RESPONSE::AuthOk { .. }));
        }
        other => panic!("expected encrypted SMSG_AUTH_RESPONSE, got {other}"),
    }

    // --- client: send an encrypted CMSG the server decrypts ---
    CMSG_CHAR_ENUM {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    drop(client);
    server.join().unwrap();
}

#[test]
fn queued_handshake_sends_wait_queue_then_admits_once_a_seat_frees() {
    let store = std::sync::Arc::new(tester_store(42));
    let queue = std::sync::Arc::new(LoginQueue::new(1, 0));
    // Occupy the only seat directly — exactly what an already-connected world session holds.
    assert_eq!(queue.request(), Admission::Admitted);

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server_queue = queue.clone();
    let server = std::thread::spawn(move || {
        let mut s = server_end;
        // Blocks in the queue's wait loop until `queue.depart()` (below) frees the one seat.
        let (conn, _encrypt) =
            world_handshake_with_queue(&mut s, server_store.as_ref(), &server_queue)
                .unwrap()
                .expect("handshake should eventually succeed once admitted");
        assert_eq!(conn.account_id, 42);
    });

    // --- client: drive the shared challenge→proof→AUTH_SESSION prefix (`drive_auth`) ---
    let (_c_enc, mut c_dec) = drive_auth(&mut client, "TESTER", K);

    // --- client: the FIRST encrypted response must be AuthWaitQueue at position 1, not AuthOk —
    // proving the handshake actually queued instead of bypassing the gate ---
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_RESPONSE(r) => {
            assert!(
                matches!(*r, SMSG_AUTH_RESPONSE::AuthWaitQueue { queue_position: 1 }),
                "expected AuthWaitQueue at position 1, got {r:?}"
            );
        }
        other => panic!("expected encrypted SMSG_AUTH_RESPONSE, got {other}"),
    }

    // Free the one seat — mirrors what `run_world_session_with_queue`'s teardown does on a real
    // disconnect. The queued handshake must notice on its next poll and proceed to admission.
    queue.depart();

    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_RESPONSE(r) => {
            assert!(
                matches!(*r, SMSG_AUTH_RESPONSE::AuthOk { .. }),
                "expected AuthOk, got {r:?}"
            );
        }
        other => panic!("expected encrypted SMSG_AUTH_RESPONSE, got {other}"),
    }

    drop(client);
    server.join().unwrap();
}

#[test]
fn disconnecting_while_queued_leaves_the_line_without_taking_a_seat() {
    let store = std::sync::Arc::new(tester_store(7));
    let queue = std::sync::Arc::new(LoginQueue::new(1, 0));
    assert_eq!(queue.request(), Admission::Admitted); // occupy the only seat

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server_queue = queue.clone();
    let server = std::thread::spawn(move || {
        let mut s = server_end;
        let result =
            world_handshake_with_queue(&mut s, server_store.as_ref(), &server_queue).unwrap();
        assert!(
            result.is_none(),
            "a hangup while queued must end the session cleanly, not error"
        );
    });

    let (_c_enc, mut c_dec) = drive_auth(&mut client, "TESTER", K);

    // Read (and discard) the first AuthWaitQueue so we know the server has actually queued us
    // before hanging up — a hangup racing the very first send would prove nothing about `cancel`.
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_RESPONSE(r) => {
            assert!(matches!(*r, SMSG_AUTH_RESPONSE::AuthWaitQueue { .. }));
        }
        other => panic!("expected AuthWaitQueue, got {other}"),
    }

    drop(client); // hang up while still queued
    server.join().unwrap();

    // The seat is still held by the ORIGINAL occupant (never touched); the queued-then-cancelled
    // connection must have left NO trace in the line.
    assert_eq!(
        queue.depth(),
        0,
        "the cancelled ticket must not linger in the queue"
    );
    assert_eq!(
        queue.active(),
        1,
        "cancelling a waiter must never grant or free a seat"
    );
}

#[test]
fn a_restarted_gateway_completes_the_handshake_from_realm_state_alone() {
    // The stateless-gateway invariant, now realm-scoped. The session key K lives in
    // `game_session` — on realm-core when it is configured, on the world DB when it is not — and
    // the gateway keeps NOTHING about it. So killing the gateway mid-session and starting a fresh
    // one must let the same account re-handshake with no re-logon, against a brand-new process
    // that has never seen this client.
    //
    // Modelled here as two handshakes against two INDEPENDENT store instances that share only the
    // realm-held session row: `store_before` is the gateway that gets killed, `store_after` is the
    // replacement. Each handshake mints its own server seed and its own client seed, so nothing
    // from the first run can be smuggled into the second — if any handshake input were
    // gateway-local rather than realm state, the second run could not succeed.
    let realm_session = || WorldSession {
        account_id: 42,
        session_key: K,
    };
    let handshake_once = |store: InMemoryStore| {
        let (mut client, server_end) = world_session_socket_pair();
        let server = std::thread::spawn(move || {
            let mut s = server_end;
            let established = world_handshake(&mut s, &store)
                .unwrap()
                .expect("handshake should succeed");
            established.0.account_id
        });
        client_handshake(&mut client, "TESTER", K);
        drop(client);
        server.join().unwrap()
    };

    let store_before = InMemoryStore {
        username: "TESTER".into(),
        session: Some(realm_session()),
        ..Default::default()
    };
    assert_eq!(handshake_once(store_before), 42);

    // ---- the gateway is killed here; every byte of its in-process state is gone ----

    let store_after = InMemoryStore {
        username: "TESTER".into(),
        session: Some(realm_session()), // re-READ from the realm, not carried over
        ..Default::default()
    };
    assert_eq!(
        handshake_once(store_after),
        42,
        "a fresh gateway must re-establish the session from realm-held state alone"
    );
}

#[test]
fn a_gateway_that_cannot_reach_the_session_store_rejects_rather_than_guessing() {
    // The other half of the stateless-gateway invariant: "resume from realm state" must not
    // degrade into "resume from anything". A store that cannot answer (realm-core unreachable →
    // `lookup_session` yields no session) rejects the handshake plaintext instead of establishing
    // a session on an unverified key. `CoordinatorStore` reaches this state by way of
    // `Coordinator::realm_core()`'s Err.
    let store = InMemoryStore {
        username: "TESTER".into(),
        session: None,
        ..Default::default()
    };
    let (mut client, server_end) = world_session_socket_pair();
    let server = std::thread::spawn(move || {
        let mut s = server_end;
        assert!(
            world_handshake(&mut s, &store).unwrap().is_none(),
            "no session material ⇒ no session, never a best-effort one"
        );
    });

    let (_enc, _dec) = drive_auth(&mut client, "TESTER", K);
    match ServerOpcodeMessage::read_unencrypted(&mut client).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_RESPONSE(r) => {
            assert!(matches!(*r, SMSG_AUTH_RESPONSE::AuthUnknownAccount));
        }
        other => panic!("expected a plaintext rejection, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn bad_proof_is_rejected() {
    // The store hands out a session key, but the client computes its proof against a
    // DIFFERENT key, so the server's digest check must fail.
    let store = InMemoryStore {
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 1,
            session_key: K,
        }),
        ..Default::default()
    };

    let (mut client, server_end) = world_session_socket_pair();
    let server = std::thread::spawn(move || {
        let mut s = server_end;
        let conn = world_handshake(&mut s, &store).unwrap();
        assert!(conn.is_none(), "bad proof must not establish a connection");
    });

    let server_seed = match ServerOpcodeMessage::read_unencrypted(&mut client).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_CHALLENGE(c) => c.server_seed,
        other => panic!("expected SMSG_AUTH_CHALLENGE, got {other}"),
    };

    let wrong_key = [0x11u8; 40];
    let client_seed = ProofSeed::new();
    let client_seed_value = client_seed.seed();
    let (client_proof, _crypto) =
        client_seed.into_client_header_crypto(&ns("TESTER"), wrong_key, server_seed);

    auth_session("TESTER", client_seed_value, client_proof)
        .write_unencrypted_client(&mut client)
        .unwrap();

    // The failure is sent plaintext (no cipher was established).
    match ServerOpcodeMessage::read_unencrypted(&mut client).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_RESPONSE(r) => {
            assert!(matches!(*r, SMSG_AUTH_RESPONSE::AuthFailed));
        }
        other => panic!("expected plaintext SMSG_AUTH_RESPONSE failure, got {other}"),
    }

    drop(client);
    server.join().unwrap();
}

#[test]
fn unknown_account_is_rejected_cleanly() {
    let store = InMemoryStore {
        username: "TESTER".into(),
        session: None, // account exists nowhere / no session
        ..Default::default()
    };

    let (mut client, server_end) = world_session_socket_pair();
    let server = std::thread::spawn(move || {
        let mut s = server_end;
        assert!(world_handshake(&mut s, &store).unwrap().is_none());
    });

    let (_enc, _dec) = drive_auth(&mut client, "NOBODY", K);

    match ServerOpcodeMessage::read_unencrypted(&mut client).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_RESPONSE(r) => {
            assert!(matches!(*r, SMSG_AUTH_RESPONSE::AuthUnknownAccount));
        }
        other => panic!("expected SMSG_AUTH_RESPONSE unknown-account, got {other}"),
    }

    drop(client);
    server.join().unwrap();
}

#[test]
fn char_enum_returns_the_seeded_character() {
    // The pre-seeded Human Warrior "Tester" must appear on the character-select screen.
    let tester = codec::CharacterView {
        guid: 1,
        name: "Tester".into(),
        race: 1,   // Human
        class: 1,  // Warrior
        gender: 0, // Male
        level: 1,
        map_id: 0,   // Eastern Kingdoms
        zone_id: 12, // Elwynn Forest
        x: -8949.95,
        y: -132.493,
        z: 83.5312,
        first_login: true,
        ..Default::default()
    };
    let store = std::sync::Arc::new(InMemoryStore {
        characters: vec![tester],
        ..tester_store(7)
    });

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.as_ref()).unwrap();
    });

    // Full handshake, then request the character list over the encrypted channel.
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_CHAR_ENUM {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_ENUM(e) => {
            assert_eq!(e.characters.len(), 1);
            let ch = &e.characters[0];
            assert_eq!(ch.name, "Tester");
            assert_eq!(ch.guid.guid(), 1);
            assert_eq!(ch.race, Race::Human);
            assert_eq!(ch.class, Class::Warrior);
            assert_eq!(ch.level, Level::new(1));
            assert_eq!(ch.map, Map::EasternKingdoms);
        }
        other => panic!("expected SMSG_CHAR_ENUM, got {other}"),
    }

    drop(client);
    server.join().unwrap();
}

#[test]
fn char_create_replies_success_then_name_in_use() {
    // The fake store reports a name already among its characters as in-use, else success.
    let tester = codec::CharacterView {
        guid: 1,
        name: "Tester".into(),
        race: 1,
        class: 1,
        level: 1,
        ..Default::default()
    };
    let store = std::sync::Arc::new(InMemoryStore {
        characters: vec![tester],
        ..tester_store(7)
    });
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.as_ref()).unwrap();
    });

    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    let mk = |name: &str| CMSG_CHAR_CREATE {
        name: name.to_string(),
        race: Race::Human,
        class: Class::Warrior,
        gender: Gender::Male,
        skin_color: 0,
        face: 0,
        hair_style: 0,
        hair_color: 0,
        facial_hair: 0,
    };

    // A fresh name → CharCreateSuccess.
    mk("Newbie")
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_CREATE(m) => {
            assert_eq!(m.result, WorldResult::CharCreateSuccess)
        }
        other => panic!("expected SMSG_CHAR_CREATE, got {other}"),
    }

    // An existing name → CharCreateNameInUse, and the session stays alive.
    mk("Tester")
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_CREATE(m) => {
            assert_eq!(m.result, WorldResult::CharCreateNameInUse)
        }
        other => panic!("expected SMSG_CHAR_CREATE, got {other}"),
    }

    drop(client);
    server.join().unwrap();
}

#[test]
fn char_delete_replies_success_and_dispatches_owned_guid() {
    let tester = codec::CharacterView {
        guid: 5,
        name: "Tester".into(),
        race: 1,
        class: 1,
        level: 1,
        ..Default::default()
    };
    let store = std::sync::Arc::new(InMemoryStore {
        characters: vec![tester],
        ..tester_store(7)
    });
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.as_ref()).unwrap();
    });

    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_CHAR_DELETE { guid: Guid::new(5) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_DELETE(m) => {
            assert_eq!(m.result, WorldResult::CharDeleteSuccess)
        }
        other => panic!("expected SMSG_CHAR_DELETE, got {other}"),
    }
    assert_eq!(*store.deleted.lock().unwrap(), vec![(7, 5)]);

    drop(client);
    server.join().unwrap();
}

#[test]
fn char_delete_failure_replies_failed_and_keeps_session_alive() {
    let store = std::sync::Arc::new(InMemoryStore {
        delete_outcome: Some(codec::CharDeleteOutcome::Failed),
        ..tester_store(7)
    });
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.as_ref()).unwrap();
    });

    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_CHAR_DELETE {
        guid: Guid::new(999),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_DELETE(m) => {
            assert_eq!(m.result, WorldResult::CharDeleteFailed)
        }
        other => panic!("expected SMSG_CHAR_DELETE, got {other}"),
    }

    // Session should still be alive: CMSG_CHAR_ENUM gets a normal reply, not a dropped socket.
    CMSG_CHAR_ENUM {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_ENUM(_) => {}
        other => panic!("expected SMSG_CHAR_ENUM, got {other}"),
    }

    drop(client);
    server.join().unwrap();
}

/// Human/Warrior entity matching the seed, as the gateway would read it back after
/// `player_login`.
fn warrior_entity() -> codec::EntityView {
    codec::EntityView {
        guid: 1,
        map_id: 0,
        instance_id: 0, // the open world
        zone_id: 12,
        x: -8949.95,
        y: -132.493,
        z: 83.5312,
        orientation: 0.0,
        last_move_ms: 0,
        movement_flags: 0,
        run_speed_mult_bp: 10_000,
        type_mask: 0x19,
        entry: 0,
        scale_x: 1.0,
        health: 60,
        max_health: 60,
        power: 0,
        max_power: 1000,
        level: 1,
        faction_template: 1,
        target_guid: 0,
        unit_bytes_0: 1 | (1 << 8) | (1 << 24), // human(1) warrior(1) male(0) rage(1)
        display_id: 49,
        native_display_id: 49,
        unit_flags: 0,
        base_attack_time_ms: 2000,
        dynamic_flags: 0,
        player_bytes: 0,
        player_bytes_2: 0,
        player_bytes_3: 0,
        player_flags: 0,
        xp: 0,
        next_level_xp: 0,
        money: 0,
        unit_bytes_1: 0,
        unit_bytes_2: 0,
        // L1 Human Warrior base attributes (cmangos curve) — non-zero so the CREATE exercises them.
        strength: 23,
        agility: 20,
        stamina: 22,
        intellect: 20,
        spirit: 21,
        npc_flags: 0,        // a player is not an NPC
        owner_guid: 0,       // not a summon
        effective_armor: 40, // agility 20 * 2 (base; no gear in the fixture → effective == base)
        // No hearthstone bind recorded for the test entity; fall back to login position.
        home_map: 0,
        home_zone: 0,
        home_x: 0.0,
        home_y: 0.0,
        home_z: 0.0,
    }
}

#[test]
fn player_login_emits_sequence_then_self_create() {
    // CMSG_PLAYER_LOGIN must yield the full login sequence (in order) followed by the
    // self CREATE_OBJECT2 at the correct position/guid.
    let store = std::sync::Arc::new(InMemoryStore {
        login_entity: Some(warrior_entity()),
        ..tester_store(7)
    });

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.as_ref()).unwrap();
    });

    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    // The 9-message login sequence, then the self CREATE_OBJECT2.
    let mut tags = Vec::new();
    let mut create_guid = None;
    for _ in 0..10 {
        match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
            ServerOpcodeMessage::SMSG_LOGIN_VERIFY_WORLD(m) => {
                tags.push("verify_world");
                assert_eq!(m.map, Map::EasternKingdoms);
                assert!((m.position.x - (-8949.95)).abs() < 0.01);
            }
            ServerOpcodeMessage::SMSG_ACCOUNT_DATA_TIMES(_) => tags.push("account_data_times"),
            ServerOpcodeMessage::SMSG_LOGIN_SETTIMESPEED(_) => tags.push("settimespeed"),
            ServerOpcodeMessage::SMSG_TUTORIAL_FLAGS(_) => tags.push("tutorial_flags"),
            ServerOpcodeMessage::SMSG_INITIAL_SPELLS(_) => tags.push("initial_spells"),
            ServerOpcodeMessage::SMSG_ACTION_BUTTONS(_) => tags.push("action_buttons"),
            ServerOpcodeMessage::SMSG_INITIALIZE_FACTIONS(_) => tags.push("init_factions"),
            ServerOpcodeMessage::SMSG_SET_REST_START(_) => tags.push("set_rest_start"),
            ServerOpcodeMessage::SMSG_BINDPOINTUPDATE(_) => tags.push("bindpoint"),
            ServerOpcodeMessage::SMSG_UPDATE_OBJECT(m) => {
                tags.push("update_object");
                if let Object::CreateObject2 { guid3, .. } = &m.objects[0] {
                    create_guid = Some(guid3.guid());
                } else {
                    panic!("expected CreateObject2 in self-spawn");
                }
            }
            other => panic!("unexpected message in login sequence: {other}"),
        }
    }

    assert_eq!(
        tags,
        vec![
            "verify_world",
            "account_data_times",
            "settimespeed",
            "tutorial_flags",
            "initial_spells",
            "action_buttons",
            "init_factions",
            "set_rest_start",
            "bindpoint",
            "update_object",
        ],
    );
    assert_eq!(create_guid, Some(1));

    drop(client);
    server.join().unwrap();
}

#[test]
fn worldport_ack_reenters_the_world_at_the_new_map_with_a_fresh_subscription() {
    // MSG_MOVE_WORLDPORT_ACK must re-run the SAME enter_world path as
    // CMSG_PLAYER_LOGIN — rebuilding the entity (now on the NEW map the module's teleport_player
    // durably wrote to the character row) and re-subscribing with a FRESH `created` dedup set — a
    // reused dedup set from the old map would suppress the initial sweep of pre-existing entities
    // through the CREATE path, leaving the new map looking empty until something moved or spawned —
    // a stale created-set is exactly what would leave entities invisible on cross-map arrival.
    let mut ported = warrior_entity();
    ported.map_id = 1; // Kalimdor — simulates teleport_player's durable cross-map write
    ported.x = 100.0;
    ported.y = 200.0;
    let store = std::sync::Arc::new(InMemoryStore {
        // A real cross-map teleport has despawned the old-map entity before its ack arrives.
        entity_in_world: false,
        login_entity: Some(warrior_entity()),
        worldport_entity: Some(ported),
        ..tester_store(7)
    });

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.as_ref()).unwrap();
    });

    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    // Drain the initial 10-message login sequence (map 0 — not the point of this test).
    for _ in 0..10 {
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    }

    // The client finished "loading" the new map (having received TRANSFER_PENDING/NEW_WORLD from the
    // on_teleport relay, which this InMemoryStore-driven dispatch test doesn't exercise — that's
    // covered by `stdb::subscriptions::tests::teleport_relay_*` and the codec pins) -> sends the
    // (empty-body) ack.
    MSG_MOVE_WORLDPORT_ACK {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    // enter_world reruns the login-style sequence for the re-entry — minus SMSG_LOGIN_VERIFY_WORLD
    // (9 messages, not 10): a verify-world resend commands a second load of the just-loaded map.
    let mut create_guid = None;
    for _ in 0..9 {
        match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
            ServerOpcodeMessage::SMSG_LOGIN_VERIFY_WORLD(_) => {
                panic!(
                    "the re-entry sequence must NOT resend SMSG_LOGIN_VERIFY_WORLD — it makes the \
                     client reload the map it just loaded (a second loading screen)"
                );
            }
            ServerOpcodeMessage::SMSG_UPDATE_OBJECT(m) => {
                if let Object::CreateObject2 { guid3, .. } = &m.objects[0] {
                    create_guid = Some(guid3.guid());
                }
            }
            _ => {}
        }
    }
    assert_eq!(
        create_guid,
        Some(1),
        "the self entity is re-created (despawn+rebuild), not a stale row"
    );

    // subscribe_player_events must have fired TWICE: once at login (map 0), once at the world-port
    // re-entry (map 1) — a fresh `created` set is built for the new map each time, never reused.
    let calls = store.subscribed.lock().unwrap().clone();
    assert_eq!(
        calls.len(),
        2,
        "subscribe_player_events must run again on WORLDPORT_ACK"
    );
    assert_eq!(calls[0].1, 0, "the first subscription is for the login map");
    assert_eq!(
        calls[1].1, 1,
        "the second subscription is for the NEW (post-teleport) map"
    );
    assert!(
        (calls[1].2 - 100.0).abs() < 0.01,
        "the re-entry must use the NEW position (verify-world no longer carries it — the \
         subscription placement is the observable)"
    );

    drop(client);
    server.join().unwrap();
}

#[test]
fn login_initialize_factions_carries_persisted_standing_at_its_reputation_index() {
    // A persisted `game_player_reputation` row must land in the login
    // SMSG_INITIALIZE_FACTIONS at its STORED reputation_index slot (0..63), never faction_id — the
    // guardrail that also gates the live SET_FACTION_STANDING relay (McBride ERROR #132).
    let store = std::sync::Arc::new(InMemoryStore {
        login_entity: Some(warrior_entity()),
        // Stormwind's rep-index is 19 (Faction.dbc ReputationListID), NOT its faction id (72) —
        // exercising the exact index/id distinction the guardrail protects.
        reputations: vec![(19, 3175, false)],
        ..tester_store(7)
    });

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.as_ref()).unwrap();
    });

    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    // Drain the full login sequence + self CREATE_OBJECT2 (mirrors the message count in
    // player_login_emits_sequence_then_self_create) so the server side doesn't see a broken pipe.
    let mut factions = None;
    for _ in 0..10 {
        if let ServerOpcodeMessage::SMSG_INITIALIZE_FACTIONS(m) =
            ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap()
        {
            factions = Some(m.factions);
        }
    }
    let factions = factions.expect("SMSG_INITIALIZE_FACTIONS not found in login sequence");
    assert_eq!(factions.len(), 64);
    assert_eq!(
        factions[19].standing, 3175,
        "slot 19 (the stored reputation_index) must carry the standing"
    );
    // Stormwind's faction_id (72) is itself past the 64-slot array — proof that indexing by faction_id
    // (the McBride bug) would panic/crash here rather than silently landing on the wrong slot.
    for (i, f) in factions.iter().enumerate() {
        if i != 19 {
            assert_eq!(f.standing, 0, "slot {i} should remain the Neutral/0 stub");
        }
    }

    drop(client);
    server.join().unwrap();
}

#[test]
fn inbound_movement_is_recorded_under_its_opcode() {
    use wow_world_messages::vanilla::{
        MSG_MOVE_HEARTBEAT_Client, MovementInfo, MovementInfo_MovementFlags, Vector3d,
    };

    let store = std::sync::Arc::new(InMemoryStore {
        entity_in_world: true,
        ..tester_store(7)
    });

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.as_ref()).unwrap();
    });

    let (mut c_enc, _c_dec) = client_handshake(&mut client, "TESTER", K);
    let info = MovementInfo {
        flags: MovementInfo_MovementFlags::empty(),
        timestamp: 12345,
        position: Vector3d {
            x: -8950.0,
            y: -130.0,
            z: 83.0,
        },
        orientation: 1.5,
        fall_time: 0.0,
    };
    MSG_MOVE_HEARTBEAT_Client { info }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    drop(client); // server reads the heartbeat, then EOF
    server.join().unwrap();

    let moves = store.moves.lock().unwrap();
    assert_eq!(moves.len(), 1);
    let (opcode, x, _, _, o, t) = moves[0];
    assert_eq!(
        opcode,
        lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT
    );
    assert!((x - (-8950.0)).abs() < 0.01);
    assert!((o - 1.5).abs() < 0.001);
    assert_eq!(t, 12345);
}

#[test]
fn a_movement_packet_for_a_despawned_entity_never_kills_the_session() {
    use wow_world_messages::vanilla::{
        MSG_MOVE_HEARTBEAT_Client, MSG_MOVE_START_FORWARD_Client, MovementInfo,
        MovementInfo_MovementFlags, Vector3d,
    };
    let calls: ShardCallLog = Default::default();
    let entity_presence = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let store = std::sync::Arc::new(InMemoryStore {
        calls: calls.clone(),
        entity_presence: Some(entity_presence.clone()),
        ..tester_store(7)
    });

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.as_ref()));

    let (mut c_enc, _c_dec) = client_handshake(&mut client, "TESTER", K);
    let beat = |t: u32| MSG_MOVE_HEARTBEAT_Client {
        info: MovementInfo {
            flags: MovementInfo_MovementFlags::empty(),
            timestamp: t,
            position: Vector3d {
                x: -8950.0,
                y: -130.0,
                z: 83.0,
            },
            orientation: 1.5,
            fall_time: 0.0,
        },
    };
    // The first packet is a transfer tail. Once the coordinator cache sees the entity again, the
    // next state transition must resume normal batched submission.
    beat(1)
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    for _ in 0..100 {
        if store
            .entity_presence_checks
            .load(std::sync::atomic::Ordering::SeqCst)
            != 0
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_ne!(
        store
            .entity_presence_checks
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the encrypted movement packet must reach the coordinator presence gate"
    );
    entity_presence.store(true, std::sync::atomic::Ordering::SeqCst);
    MSG_MOVE_START_FORWARD_Client { info: beat(2).info }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drop(client);

    server.join().unwrap().expect(
        "a movement packet for a despawned entity must be DROPPED, not session-fatal — \
                 closing the socket here strands the client on a loading screen with no error",
    );
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, c)| c == "movement_update")
            .count(),
        1,
        "the transfer-tail packet must not enter the batch, while movement resumes when presence returns"
    );
}

#[test]
fn a_reappearing_entity_resets_the_movement_desync_tolerance() {
    use wow_world_messages::vanilla::{
        MSG_MOVE_START_FORWARD_Client, MSG_MOVE_STOP_Client, MovementInfo,
        MovementInfo_MovementFlags, Vector3d,
    };
    let present = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let store = std::sync::Arc::new(InMemoryStore {
        entity_presence: Some(present.clone()),
        ..tester_store(7)
    });
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.as_ref()));
    let (mut c_enc, _c_dec) = client_handshake(&mut client, "TESTER", K);
    let info = |t| MovementInfo {
        flags: MovementInfo_MovementFlags::empty(),
        timestamp: t,
        position: Vector3d {
            x: -8950.0,
            y: -130.0,
            z: 83.0,
        },
        orientation: 1.5,
        fall_time: 0.0,
    };
    let send_tail = |start: u32, end: u32, client: &mut UnixStream, enc: &mut EncrypterHalf| {
        for t in start..end {
            let sent = if t % 2 == 0 {
                MSG_MOVE_START_FORWARD_Client { info: info(t) }
                    .write_encrypted_client(&mut *client, enc)
            } else {
                MSG_MOVE_STOP_Client { info: info(t) }.write_encrypted_client(&mut *client, enc)
            };
            sent.unwrap();
        }
    };
    let wait_for_checks = |n| {
        for _ in 0..100 {
            if store
                .entity_presence_checks
                .load(std::sync::atomic::Ordering::SeqCst)
                >= n
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("the gateway did not check entity presence {n} times");
    };

    send_tail(0, MOVE_DESYNC_TOLERANCE, &mut client, &mut c_enc);
    wait_for_checks(MOVE_DESYNC_TOLERANCE as usize);
    present.store(true, std::sync::atomic::Ordering::SeqCst);
    MSG_MOVE_START_FORWARD_Client {
        info: info(MOVE_DESYNC_TOLERANCE),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    wait_for_checks(MOVE_DESYNC_TOLERANCE as usize + 1);
    present.store(false, std::sync::atomic::Ordering::SeqCst);
    send_tail(
        MOVE_DESYNC_TOLERANCE + 1,
        MOVE_DESYNC_TOLERANCE * 2 + 1,
        &mut client,
        &mut c_enc,
    );
    wait_for_checks(MOVE_DESYNC_TOLERANCE as usize * 2 + 1);
    present.store(true, std::sync::atomic::Ordering::SeqCst);
    MSG_MOVE_STOP_Client {
        info: info(MOVE_DESYNC_TOLERANCE * 2 + 1),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server
        .join()
        .unwrap()
        .expect("a restored entity must reset the tolerance before a later transfer tail arrives");
    assert_eq!(
        store.moves.lock().unwrap().len(),
        2,
        "only movements sent while the entity was present may reach the shared batch"
    );
}

#[test]
fn a_movement_failure_that_is_not_a_desync_is_still_session_fatal() {
    use wow_world_messages::vanilla::{
        MSG_MOVE_HEARTBEAT_Client, MovementInfo, MovementInfo_MovementFlags, Vector3d,
    };
    let store = std::sync::Arc::new(InMemoryStore {
        entity_in_world: true,
        movement_error: Some("timed out after 10s".into()),
        ..tester_store(7)
    });
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.as_ref()));
    let (mut c_enc, _c_dec) = client_handshake(&mut client, "TESTER", K);
    MSG_MOVE_HEARTBEAT_Client {
        info: MovementInfo {
            flags: MovementInfo_MovementFlags::empty(),
            timestamp: 1,
            position: Vector3d {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            orientation: 0.0,
            fall_time: 0.0,
        },
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    let err = server
        .join()
        .unwrap()
        .expect_err("a non-desync movement failure must still end the session");
    assert!(format!("{err:#}").contains("timed out"), "{err:#}");
}

#[test]
fn a_movement_desync_that_never_heals_still_ends_the_session() {
    use wow_world_messages::vanilla::{
        MSG_MOVE_START_FORWARD_Client, MSG_MOVE_STOP_Client, MovementInfo,
        MovementInfo_MovementFlags, Vector3d,
    };
    let store = std::sync::Arc::new(InMemoryStore {
        // The entity is gone and is NEVER coming back — not a teleport tail, a real desync.
        entity_in_world: false,
        ..tester_store(7)
    });
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.as_ref()));
    let (mut c_enc, _c_dec) = client_handshake(&mut client, "TESTER", K);
    let info = |t: u32| MovementInfo {
        flags: MovementInfo_MovementFlags::empty(),
        timestamp: t,
        position: Vector3d {
            x: -8950.0,
            y: -130.0,
            z: 83.0,
        },
        orientation: 1.5,
        fall_time: 0.0,
    };
    // State transitions, so every one forwards immediately (never coalesced). Far more than a
    // cross-map port's in-flight tail, and spread over more time than any loading screen's worth of
    // queued packets — a session still serving these has stopped detecting desyncs altogether.
    // Writes are best-effort: the socket SHOULD close partway through, which is the whole point.
    for i in 0..200u32 {
        let ok = if i % 2 == 0 {
            MSG_MOVE_START_FORWARD_Client { info: info(i) }
                .write_encrypted_client(&mut client, &mut c_enc)
                .is_ok()
        } else {
            MSG_MOVE_STOP_Client { info: info(i) }
                .write_encrypted_client(&mut client, &mut c_enc)
                .is_ok()
        };
        if !ok {
            break;
        }
    }
    drop(client);
    let err = server.join().unwrap().expect_err(
        "a movement desync that never heals must STILL end the session: tolerating the tail of a \
         cross-map port is one thing, serving a permanently desynced client a frozen world forever \
         is the hang a stuck cross-map transfer causes, with no loading screen to blame it on",
    );
    assert!(format!("{err:#}").contains("not in world"), "{err:#}");
}

#[test]
fn a_world_port_whose_transfer_cannot_be_driven_aborts_the_clients_loading_screen() {
    let xdb = FakeShardDb::with_character(
        1,
        FakeChar {
            map_id: 36,
            instance_id: 7,
            payload: "gear+spells".into(),
        },
    );
    let store = std::sync::Arc::new(InMemoryStore {
        // The world-port ack is only actionable after teleport_player removed the old entity.
        entity_in_world: false,
        characters: vec![],
        login_entity: Some(warrior_entity()),
        xdb: Some(xdb),
        settle_error: Some("instances shard unreachable".into()),
        settle_ok_calls: 1, // the LOGIN routes fine; the world-port's settle is the one that fails
        ..tester_store(7)
    });

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.as_ref()));
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    for _ in 0..10 {
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    }
    // The client finished loading the dungeon map and acks — this is where the transfer runs.
    MSG_MOVE_WORLDPORT_ACK {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    let aborted = ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).expect(
        "the client must receive SMSG_TRANSFER_ABORTED — silence here is the loading-screen hang this fixes",
    );
    match aborted {
        ServerOpcodeMessage::SMSG_TRANSFER_ABORTED(m) => {
            assert_eq!(
                m.map,
                Map::Deadmines,
                "the abort must name the map the client is loading"
            );
        }
        other => panic!("expected SMSG_TRANSFER_ABORTED, got {other}"),
    }
    drop(client);
    let err = server.join().unwrap().expect_err(
        "a half-driven transfer must still end the session rather than enter the world",
    );
    assert!(
        format!("{err:#}").contains("instances shard unreachable"),
        "{err:#}"
    );
}

#[test]
fn a_world_port_whose_world_entry_fails_also_aborts_the_clients_loading_screen() {
    let xdb = FakeShardDb::with_character(
        1,
        FakeChar {
            map_id: 36,
            instance_id: 7,
            payload: "gear+spells".into(),
        },
    );
    let store = std::sync::Arc::new(InMemoryStore {
        // The world-port ack is only actionable after teleport_player removed the old entity.
        entity_in_world: false,
        characters: vec![],
        login_entity: Some(warrior_entity()),
        xdb: Some(xdb),
        // Routing succeeds; the world entry on the far side is what fails.
        worldport_login_error: Some("character 1 is stranded on map 36".into()),
        ..tester_store(7)
    });

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.as_ref()));
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    for _ in 0..10 {
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    }
    MSG_MOVE_WORLDPORT_ACK {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    // Drain whatever the (partial) re-entry emitted and require an abort somewhere in it — the
    // client must not be left loading. `read_encrypted` returns Err once the socket closes.
    let mut aborted = None;
    while let Ok(msg) = ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec) {
        if let ServerOpcodeMessage::SMSG_TRANSFER_ABORTED(m) = msg {
            aborted = Some(m);
            break;
        }
    }
    let m = aborted.expect(
        "a world-port whose ENTRY fails must still abort the client's loading screen — silence \
         here is the infinite loading bar this abort exists to kill",
    );
    assert_eq!(
        m.map,
        Map::Deadmines,
        "the abort must name the map the client is loading"
    );
    drop(client);
    let err = server
        .join()
        .unwrap()
        .expect_err("a half-entered world-port must still end the session");
    assert!(format!("{err:#}").contains("stranded"), "{err:#}");
}

#[test]
fn movement_state_changes_forward_immediately_and_in_order_never_delayed() {
    use wow_world_messages::vanilla::{
        MSG_MOVE_HEARTBEAT_Client, MSG_MOVE_START_FORWARD_Client, MSG_MOVE_START_TURN_LEFT_Client,
        MSG_MOVE_STOP_Client, MovementInfo, MovementInfo_MovementFlags, Vector3d,
    };

    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);

    let mk = |flags: u32, orientation: f32, x: f32, timestamp: u32| MovementInfo {
        flags: MovementInfo_MovementFlags::new(flags, None, None, None, None),
        timestamp,
        position: Vector3d { x, y: 0.0, z: 0.0 },
        orientation,
        fall_time: 0.0,
    };
    const RUN: u32 = 0x1;
    const TURN: u32 = 0x1 | 0x10;
    const STOPPED: u32 = 0x0;

    MSG_MOVE_START_FORWARD_Client {
        info: mk(RUN, 0.0, 0.0, 100),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // Same flags + heading as the run-start -> a pure heartbeat -> held pending (nowhere near the
    // 150ms window in real wall-clock terms, since these all write back-to-back with no sleep).
    MSG_MOVE_HEARTBEAT_Client {
        info: mk(RUN, 0.0, 5.0, 200),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // A second same-vector heartbeat SUPERSEDES the first (rule 3: the pending slot IS the drop
    // mechanism) — x=5.0 must never reach the module.
    MSG_MOVE_HEARTBEAT_Client {
        info: mk(RUN, 0.0, 10.0, 300),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // A turn: different flags -> a STATE CHANGE. Must flush the pending x=10.0 heartbeat FIRST,
    // then forward the turn itself, both undelayed.
    MSG_MOVE_START_TURN_LEFT_Client {
        info: mk(TURN, 5.0, 10.0, 400),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // A stop: another STATE CHANGE, no pending left to flush — forwards immediately alone.
    MSG_MOVE_STOP_Client {
        info: mk(STOPPED, 5.0, 10.0, 500),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();

    drop(client);
    server.join().unwrap();

    let moves = store.moves.lock().unwrap();
    let opcodes: Vec<u32> = moves.iter().map(|(op, ..)| *op).collect();
    assert_eq!(
        opcodes,
        vec![
            lyracore_shared::opcodes::movement::MSG_MOVE_START_FORWARD,
            lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT, // the FLUSHED x=10.0 heartbeat
            lyracore_shared::opcodes::movement::MSG_MOVE_START_TURN_LEFT,
            lyracore_shared::opcodes::movement::MSG_MOVE_STOP,
        ],
        "run-start, [flushed heartbeat], turn, stop — in that exact order; the turn and stop must \
         never be held, and the superseded x=5.0 intermediate must never appear at all"
    );
    // The flushed heartbeat carries the LATEST pending position (x=10.0), not the dropped x=5.0 one.
    assert!(
        (moves[1].1 - 10.0).abs() < 0.01,
        "flushed heartbeat must carry the superseding x=10.0, not the dropped x=5.0"
    );
    assert_eq!(
        moves.len(),
        4,
        "exactly one heartbeat survives coalescing out of the two sent"
    );
}

#[test]
fn non_movement_opcode_flushes_pending_heartbeat_before_being_handled() {
    use wow_world_messages::vanilla::{
        MSG_MOVE_HEARTBEAT_Client, MSG_MOVE_START_FORWARD_Client, MovementInfo,
        MovementInfo_MovementFlags, Vector3d,
    };

    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    let mk = |x: f32, timestamp: u32| MovementInfo {
        flags: MovementInfo_MovementFlags::new(0x1, None, None, None, None),
        timestamp,
        position: Vector3d { x, y: 0.0, z: 0.0 },
        orientation: 0.0,
        fall_time: 0.0,
    };

    MSG_MOVE_START_FORWARD_Client { info: mk(0.0, 100) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    // Three same-vector heartbeats, sent back-to-back (well inside the 150ms window): each
    // supersedes the last in the pending slot, so only the FINAL one (x=30.0) may ever ship.
    MSG_MOVE_HEARTBEAT_Client {
        info: mk(10.0, 200),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    MSG_MOVE_HEARTBEAT_Client {
        info: mk(20.0, 300),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    MSG_MOVE_HEARTBEAT_Client {
        info: mk(30.0, 400),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();

    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {} // the query itself was still answered normally
        other => panic!("expected SMSG_QUESTGIVER_STATUS, got {other}"),
    }
    // Safe to inspect now: the reply we just read could only have been produced AFTER the query's
    // dispatch ran (flush, then the query handler) on the single-threaded reader/dispatch loop.
    {
        let moves = store.moves.lock().unwrap();
        assert_eq!(
            moves.len(),
            2,
            "baseline + exactly ONE flushed heartbeat — if coalescing weren't happening, all 3 \
             heartbeats would have forwarded individually (4 total), not 2"
        );
        assert!((moves[1].1 - 30.0).abs() < 0.01, "the flushed heartbeat must carry the LATEST superseding position, not an earlier dropped one");
    }

    drop(client);
    server.join().unwrap();
}

#[test]
fn desync_error_classifies_entity_missing_as_fatal_but_not_transient() {
    // The module's `entity_by_owner` failures (the player's entity is gone — a desync, e.g. a
    // schema-change publish dropped the gateway's read subscription) must be session-FATAL so the
    // handler forces a CLEAN disconnect instead of leaving a silent zombie (can't attack / can't logout).
    assert!(is_desync_error(&anyhow!("attacker not in world")));
    assert!(is_desync_error(&anyhow!("caster not in world")));
    assert!(is_desync_error(&anyhow!("no live entity for guid 5")));
    assert!(is_desync_error(&anyhow!("Player NOT IN WORLD"))); // case-insensitive
                                                               // TRANSIENT per-action failures are NOT desync — they stay swallowed (the player keeps playing,
                                                               // never disconnected). A false positive here would drop players on every dead-target swing.
    assert!(!is_desync_error(&anyhow!("target is dead")));
    assert!(!is_desync_error(&anyhow!(
        "target out of range (35.0 yd > 30 yd)"
    )));
    assert!(!is_desync_error(&anyhow!(
        "not enough power: have 0, need 30"
    )));
    assert!(!is_desync_error(&anyhow!(
        "spell not ready (global cooldown)"
    )));
    assert!(!is_desync_error(&anyhow!("cannot attack self")));
}

// ===========================================================================================
//  Quest-giver dispatch (E2E over the world session) — the #1 documented test gap. Each test drives a
//  CMSG_QUESTGIVER_* / CMSG_QUESTLOG_REMOVE_QUEST through `run_world_session` (full handshake + login +
//  encrypted dispatch) and asserts the gateway routes it to the right `WorldStore` reducer with the right
//  args (recorded by the fake) and/or replies with the right `SMSG_QUESTGIVER_*` — the wire-to-store seam.
// ===========================================================================================

/// A store configured for an in-world TESTER (account 7, char guid 1) with a login entity — the
/// fixture every quest test builds on, then overlays quest evals / details / log slots.
fn quest_store() -> InMemoryStore {
    InMemoryStore {
        login_entity: Some(warrior_entity()),
        ..tester_store(7)
    }
}

/// A minimal quest detail (everything zero/empty but the id + title) — enough for the build_* codecs.
fn detail_view(quest_id: u32, title: &str) -> codec::QuestDetailView {
    codec::QuestDetailView {
        quest_id,
        quest_level: 1,
        zone_or_sort: 12,
        title: title.into(),
        details: String::new(),
        objectives_text: String::new(),
        offer_reward_text: String::new(),
        request_items_text: String::new(),
        money_reward: 0,
        reward_xp: 0,
        next_quest_id: 0,
        max_level_money_reward: 0,
        rewards: Vec::new(),
        choice_rewards: Vec::new(),
        objectives: Vec::new(),
    }
}

/// One giver↔quest eval with the four booleans the menu/status + complete-branch read.
fn eval(quest_id: u32, role: u8, active: bool, complete: bool) -> codec::GiverQuestEval {
    codec::GiverQuestEval {
        quest_id,
        title: "Q".into(),
        level: 1,
        role,
        startable: role == codec::ROLE_START,
        active,
        complete,
    }
}

/// Spin up a world session over a socket pair, handshake as TESTER, enter the world as `guid`, and
/// drain the login sequence — 10 fixed messages (LYRACORE_QUEST_LOG off in tests → no quest-log update
/// appended) plus one SMSG_UPDATE_OBJECT CREATE per `player_items()` row (`enter_world` inserts
/// one per owned item BEFORE the self-spawn CREATE — see its doc comment in `mod.rs`). Every
/// earlier test leaves `player_items_fixture` empty, so this reads exactly the same 10 messages
/// as before; only a test that seeds items (the CMSG_SELL_ITEM/CMSG_REPAIR_ITEM guid→slot ones)
/// needs the extra drain, and getting it wrong here manifests as the CLIENT closing with unread
/// bytes still queued — which the kernel reports back to the SERVER thread's next read as
/// ECONNRESET, not a clean EOF.
/// Returns the client socket + encrypted halves + the server join handle for the test to drive.
fn enter_world(
    store: std::sync::Arc<InMemoryStore>,
    guid: u64,
) -> (
    UnixStream,
    EncrypterHalf,
    DecrypterHalf,
    std::thread::JoinHandle<()>,
) {
    let (mut client, server_end) = world_session_socket_pair();
    // The login sequence ends with the quest-log VALUES packet IFF the player has quests (mirrors
    // `send_quest_log`'s skip-when-empty). Checked before `store` is moved into the server thread.
    let has_quest_log = store.player_quest_log(guid).is_ok_and(|s| !s.is_empty());
    let item_creates = store.player_items(guid).map(|v| v.len()).unwrap_or(0);
    let server_store = store;
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.as_ref()).unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN {
        guid: Guid::new(guid),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    for _ in 0..10 + item_creates {
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    }
    if has_quest_log {
        // The quest-log packet is a PARTIAL VALUES update with OBJECT_FIELD_TYPE stripped (so the real
        // 5875 client doesn't crash — see the health-VALUES note). gtker's DECODER rejects that ("Missing
        // object TYPE"), but the frame bytes are consumed, so drain it tolerantly rather than unwrap.
        let _ = ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec);
    }
    (client, c_enc, c_dec, server)
}

#[test]
fn the_writer_drains_the_egress_depth_back_to_zero_b2() {
    let store = std::sync::Arc::new(quest_store());
    let (client, _c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    let depth = store
        .session_depth
        .lock()
        .unwrap()
        .clone()
        .expect("the session must have handed its egress handle to subscribe_player_events");
    assert_eq!(
        depth.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "the login sequence has been fully read by the client, so the writer must have decremented \
         the egress depth for every item it wrote — a depth that only climbs makes every session \
         shed peer movement forever once it passes EGRESS_SHED_DEPTH"
    );
    drop(client);
    let _ = server.join();
}

#[test]
fn group_invite_by_name_replies_party_command_result_success() {
    // CMSG_GROUP_INVITE "Buddy" resolves the name, calls the store, and echoes
    // SMSG_PARTY_COMMAND_RESULT(Invite, "Buddy", Success); the store recorded the resolved guid.
    let mut s = quest_store();
    s.characters = vec![codec::CharacterView {
        guid: 2,
        name: "Buddy".into(),
        class: 1,
        level: 10,
        zone_id: 12,
        ..Default::default()
    }];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    wow_world_messages::vanilla::CMSG_GROUP_INVITE {
        name: "buddy".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_PARTY_COMMAND_RESULT(r) => {
            assert_eq!(r.result, wow_world_messages::vanilla::PartyResult::Success);
            assert_eq!(
                r.operation,
                wow_world_messages::vanilla::PartyOperation::Invite
            );
            assert_eq!(r.member, "buddy");
        }
        other => panic!("expected SMSG_PARTY_COMMAND_RESULT, got {other}"),
    }
    assert_eq!(store.group_invites.lock().unwrap().as_slice(), &[2]);
    drop(client);
    let _ = server.join();
}

#[test]
fn group_invite_unknown_name_replies_bad_player_name() {
    // An unresolvable name never reaches the store — the reply is BadPlayerName ("player not found").
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    wow_world_messages::vanilla::CMSG_GROUP_INVITE {
        name: "Nobody".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_PARTY_COMMAND_RESULT(r) => {
            assert_eq!(
                r.result,
                wow_world_messages::vanilla::PartyResult::BadPlayerName
            );
        }
        other => panic!("expected SMSG_PARTY_COMMAND_RESULT, got {other}"),
    }
    assert!(store.group_invites.lock().unwrap().is_empty());
    drop(client);
    let _ = server.join();
}

#[test]
fn add_friend_by_name_then_friend_list_carries_online_presence() {
    // CMSG_ADD_FRIEND "Buddy" -> SMSG_FRIEND_STATUS AddedOnline (guid 2, resolved by
    // name); a follow-up CMSG_FRIEND_LIST then carries them online with their level/class/zone.
    let mut s = quest_store();
    s.characters = vec![codec::CharacterView {
        guid: 2,
        name: "Buddy".into(),
        class: 1,
        level: 10,
        zone_id: 12,
        ..Default::default()
    }];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    CMSG_ADD_FRIEND {
        name: "buddy".into(),
    } // case-insensitive match
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_FRIEND_STATUS(s) => {
            assert_eq!(s.result, FriendResult::AddedOnline);
            assert_eq!(s.guid.guid(), 2);
        }
        other => panic!("expected SMSG_FRIEND_STATUS, got {other}"),
    }

    CMSG_FRIEND_LIST {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_FRIEND_LIST(l) => {
            assert_eq!(l.friends.len(), 1);
            assert_eq!(l.friends[0].guid.guid(), 2);
            assert!(matches!(
                l.friends[0].status,
                wow_world_messages::vanilla::Friend_FriendStatus::Online { .. }
            ));
        }
        other => panic!("expected SMSG_FRIEND_LIST, got {other}"),
    }
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_IGNORE_LIST(l) => assert!(l.ignored.is_empty()),
        other => panic!("expected SMSG_IGNORE_LIST, got {other}"),
    }

    // Removing it replies Removed and the friend list empties out.
    CMSG_DEL_FRIEND { guid: Guid::new(2) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_FRIEND_STATUS(s) => assert_eq!(s.result, FriendResult::Removed),
        other => panic!("expected SMSG_FRIEND_STATUS, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn add_friend_unknown_name_replies_not_found() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_ADD_FRIEND {
        name: "Nobody".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_FRIEND_STATUS(s) => {
            assert_eq!(s.result, FriendResult::NotFound);
            assert_eq!(s.guid.guid(), 0);
        }
        other => panic!("expected SMSG_FRIEND_STATUS, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn add_ignore_by_name_replies_ignore_added() {
    let mut s = quest_store();
    s.characters = vec![codec::CharacterView {
        guid: 3,
        name: "Pest".into(),
        ..Default::default()
    }];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_ADD_IGNORE {
        name: "Pest".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_FRIEND_STATUS(s) => {
            assert_eq!(s.result, FriendResult::IgnoreAdded);
            assert_eq!(s.guid.guid(), 3);
        }
        other => panic!("expected SMSG_FRIEND_STATUS, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn add_friend_maps_self_already_and_full_errors() {
    for (err, want) in [
        ("cannot add yourself", FriendResult::SelfX),
        ("already added", FriendResult::Already),
        ("list full", FriendResult::ListFull),
    ] {
        let mut s = quest_store();
        s.characters = vec![codec::CharacterView {
            guid: 2,
            name: "Buddy".into(),
            ..Default::default()
        }];
        s.trade_error = Some(err.into());
        let store = std::sync::Arc::new(s);
        let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
        CMSG_ADD_FRIEND {
            name: "Buddy".into(),
        }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
        match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
            ServerOpcodeMessage::SMSG_FRIEND_STATUS(s) => {
                assert_eq!(s.result, want, "store error {err:?} must map to {want:?}")
            }
            other => panic!("expected SMSG_FRIEND_STATUS, got {other}"),
        }
        drop(client);
        server.join().unwrap();
    }
}

#[test]
fn add_ignore_maps_already_and_full_errors_to_the_ignore_variants() {
    for (err, want) in [
        ("already added", FriendResult::IgnoreAlready),
        ("list full", FriendResult::IgnoreFull),
    ] {
        let mut s = quest_store();
        s.characters = vec![codec::CharacterView {
            guid: 3,
            name: "Pest".into(),
            ..Default::default()
        }];
        s.trade_error = Some(err.into());
        let store = std::sync::Arc::new(s);
        let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
        CMSG_ADD_IGNORE {
            name: "Pest".into(),
        }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
        match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
            ServerOpcodeMessage::SMSG_FRIEND_STATUS(s) => {
                assert_eq!(s.result, want, "store error {err:?} must map to {want:?}")
            }
            other => panic!("expected SMSG_FRIEND_STATUS, got {other}"),
        }
        drop(client);
        server.join().unwrap();
    }
}

#[test]
fn del_friend_unknown_contact_replies_not_found() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_DEL_FRIEND {
        guid: Guid::new(404),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_FRIEND_STATUS(s) => assert_eq!(s.result, FriendResult::NotFound),
        other => panic!("expected SMSG_FRIEND_STATUS, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn del_ignore_round_trips_added_then_unknown_is_ignore_not_found() {
    let mut s = quest_store();
    s.characters = vec![codec::CharacterView {
        guid: 3,
        name: "Pest".into(),
        ..Default::default()
    }];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);

    CMSG_ADD_IGNORE {
        name: "Pest".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_FRIEND_STATUS(s) => {
            assert_eq!(s.result, FriendResult::IgnoreAdded)
        }
        other => panic!("expected SMSG_FRIEND_STATUS, got {other}"),
    }

    // Removing the just-added contact: IgnoreRemoved.
    CMSG_DEL_IGNORE { guid: Guid::new(3) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_FRIEND_STATUS(s) => {
            assert_eq!(s.result, FriendResult::IgnoreRemoved)
        }
        other => panic!("expected SMSG_FRIEND_STATUS, got {other}"),
    }
    // Removing it again: no longer on the list -> IgnoreNotFound.
    CMSG_DEL_IGNORE { guid: Guid::new(3) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_FRIEND_STATUS(s) => {
            assert_eq!(s.result, FriendResult::IgnoreNotFound)
        }
        other => panic!("expected SMSG_FRIEND_STATUS, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn quest_accept_dispatches_to_reducer_with_giver_and_quest() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_QUESTGIVER_ACCEPT_QUEST {
        guid: Guid::new(50),
        quest_id: 1234,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client); // accept sends no SMSG — server reads it, then EOF
    server.join().unwrap();
    // The gateway resolved account 7 (from the session) + giver 50 + quest 1234 and called the reducer.
    assert_eq!(store.accepted.lock().unwrap().as_slice(), &[(7, 50, 1234)]);
}

#[test]
fn quest_hello_replies_with_the_quest_list() {
    let mut s = quest_store();
    s.quest_evals = vec![eval(1234, codec::ROLE_START, false, false)];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_QUESTGIVER_HELLO {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_QUEST_LIST(_) => {} // dispatched HELLO → quest list
        other => panic!("expected SMSG_QUESTGIVER_QUEST_LIST, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn quest_choose_reward_turns_in_and_replies_complete() {
    let mut s = quest_store();
    s.quest_details = vec![detail_view(1234, "A Threat Within")];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    // reward: 2 — the chosen pick-1-of-N slot must be threaded CMSG -> handler -> store unchanged.
    CMSG_QUESTGIVER_CHOOSE_REWARD {
        guid: Guid::new(50),
        quest_id: 1234,
        reward: 2,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // Success → the "Quest Complete" popup (the reward screen close-out).
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_QUEST_COMPLETE(_) => {}
        other => panic!("expected SMSG_QUESTGIVER_QUEST_COMPLETE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.turned_in.lock().unwrap().as_slice(),
        &[(7, 50, 1234, 2)]
    );
}

#[test]
fn quest_complete_picks_offer_reward_vs_request_items_by_completion() {
    // COMPLETE → OFFER_REWARD when the giver's END eval is complete.
    for (complete, want_offer) in [(true, true), (false, false)] {
        let mut s = quest_store();
        s.quest_details = vec![detail_view(1234, "A Threat Within")];
        s.quest_evals = vec![eval(1234, codec::ROLE_END, true, complete)];
        let store = std::sync::Arc::new(s);
        let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
        CMSG_QUESTGIVER_COMPLETE_QUEST {
            guid: Guid::new(50),
            quest_id: 1234,
        }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
        match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
            ServerOpcodeMessage::SMSG_QUESTGIVER_OFFER_REWARD(_) => {
                assert!(want_offer, "got OFFER but wanted REQUEST")
            }
            ServerOpcodeMessage::SMSG_QUESTGIVER_REQUEST_ITEMS(_) => {
                assert!(!want_offer, "got REQUEST but wanted OFFER")
            }
            other => panic!("expected OFFER_REWARD/REQUEST_ITEMS, got {other}"),
        }
        drop(client);
        server.join().unwrap();
    }
}

#[test]
fn quest_abandon_resolves_log_slot_to_quest_id() {
    let mut s = quest_store();
    // The client sends a LOG SLOT (3), not a quest id — the gateway must resolve it via player_quest_log.
    s.quest_log_slots = vec![codec::update_mask::QuestLogSlot {
        slot: 3,
        quest_id: 777,
        counts: Vec::new(),
        state: 0,
        timer: 0,
    }];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_QUESTLOG_REMOVE_QUEST { slot: 3 }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drop(client);
    server.join().unwrap();
    // Slot 3 → quest 777, abandoned for account 7.
    assert_eq!(store.abandoned.lock().unwrap().as_slice(), &[(7, 777)]);
}

#[test]
fn quest_abandon_unknown_slot_is_a_noop() {
    let mut s = quest_store();
    s.quest_log_slots = vec![codec::update_mask::QuestLogSlot {
        slot: 3,
        quest_id: 777,
        counts: Vec::new(),
        state: 0,
        timer: 0,
    }];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_QUESTLOG_REMOVE_QUEST { slot: 9 } // no such slot → resolve finds nothing → no reducer call
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drop(client);
    server.join().unwrap();
    assert!(store.abandoned.lock().unwrap().is_empty());
}

// ── Inspect ───────────────────────────────────────────────────────────────────────────────────────

#[test]
fn inspect_in_range_friendly_target_replies_smsg_inspect_with_the_target_guid() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_INSPECT {
        guid: Guid::new(55),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_INSPECT(r) => assert_eq!(r.guid.guid(), 55),
        other => panic!("expected SMSG_INSPECT, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn inspect_refused_target_sends_no_reply() {
    // CMSG_PLAYED_TIME (below) always replies as long as `character_by_guid` resolves the caller's
    // own guid, so give the store a character row for guid 1 (quest_store() has none).
    let store = std::sync::Arc::new(InMemoryStore {
        characters: vec![codec::CharacterView {
            guid: 1,
            ..Default::default()
        }],
        ..quest_store()
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    // guid 0 is the mock store's stand-in for "out of range / no such target" — the gate rejects it
    // and the handler drops the request silently (mirrors CMSG_GAMEOBJ_USE/CMSG_AREATRIGGER).
    CMSG_INSPECT { guid: Guid::new(0) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    // Sentinel: a follow-up request with a guaranteed reply. If the refused CMSG_INSPECT had
    // wrongly produced an SMSG_INSPECT, it would arrive first and this match would fail.
    CMSG_PLAYED_TIME {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_PLAYED_TIME(_) => {} // no SMSG_INSPECT was sent for the refused target
        other => {
            panic!("expected SMSG_PLAYED_TIME (no SMSG_INSPECT for refused target), got {other}")
        }
    }
    drop(client);
    server.join().unwrap();
}

// ── Vendor / buy-failed ─────────────────────────────────────────────────────────

#[test]
fn buy_item_err_sends_smsg_buy_failed() {
    // When `buy_item` returns Err (e.g. "not enough money"), the gateway must send SMSG_BUY_FAILED
    // with the matching BuyResult code so the player gets an on-screen error.
    let store = std::sync::Arc::new(InMemoryStore {
        login_entity: Some(warrior_entity()),
        trade_error: Some("not enough money to buy that item".into()),
        ..tester_store(7)
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_BUY_ITEM {
        vendor: Guid::new(99),
        item: 1234,
        amount: 1,
        unknown1: 1,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_BUY_FAILED(p) => {
            assert_eq!(p.guid.guid(), 99, "vendor guid echoed back");
            assert_eq!(p.item, 1234, "item entry echoed back");
            assert!(
                matches!(p.result, BuyResult::NotEnoughMoney),
                "BuyResult maps to NotEnoughMoney"
            );
        }
        other => panic!("expected SMSG_BUY_FAILED, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

// ── Bank ──────────────────────────────────────────────────────────────────────────

#[test]
fn banker_activate_sends_smsg_show_bank_with_the_banker_guid() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_BANKER_ACTIVATE {
        guid: Guid::new(77),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_SHOW_BANK(p) => assert_eq!(p.guid.guid(), 77),
        other => panic!("expected SMSG_SHOW_BANK, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn banker_activate_on_a_standing_refusing_banker_sends_no_reply() {
    // CMSG_PLAYED_TIME (the sentinel below) only replies once `character_by_guid` resolves the
    // caller's own guid, so give the store a character row for guid 1 (quest_store() has none) —
    // same setup as `inspect_refused_target_sends_no_reply`.
    let store = std::sync::Arc::new(InMemoryStore {
        npc_refuses: true,
        characters: vec![codec::CharacterView {
            guid: 1,
            ..Default::default()
        }],
        ..quest_store()
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_BANKER_ACTIVATE {
        guid: Guid::new(77),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // Sentinel: a follow-up request with a guaranteed reply. If the refused activate had wrongly
    // produced an SMSG_SHOW_BANK, it would arrive first and this match would fail.
    CMSG_PLAYED_TIME {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_PLAYED_TIME(_) => {} // no SMSG_SHOW_BANK for the refused banker
        other => {
            panic!("expected SMSG_PLAYED_TIME (no SMSG_SHOW_BANK for refused banker), got {other}")
        }
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn gossip_select_on_an_imported_banker_option_opens_the_bank_window() {
    use lyracore_shared::constants::gossip_option;
    let mut s = quest_store();
    s.gossip_opts = vec![opt(
        0,
        "I would like to check my deposit box.",
        gossip_option::BANKER,
    )];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    gossip_hello(&mut client, &mut c_enc, &mut c_dec, 90);
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(90),
        gossip_list_id: 0,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_SHOW_BANK(p) => assert_eq!(p.guid.guid(), 90),
        other => panic!("expected SMSG_SHOW_BANK, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn autobank_item_from_the_main_bag_dispatches_auto_bank_item() {
    // Right-click a bag item with the bank open (CMSG_AUTOBANK_ITEM) → the gateway names the source
    // slot and lets the module resolve the free bank slot; deposit and withdraw share one store method.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_AUTOBANK_ITEM {
        bag_index: 255,
        slot_index: 23,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(store.auto_banked_items.lock().unwrap().as_slice(), &[23]);
}

#[test]
fn autostore_bank_item_from_the_main_bag_dispatches_auto_bank_item() {
    // Right-click a banked item (CMSG_AUTOSTORE_BANK_ITEM) → withdraw, same store method as deposit.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_AUTOSTORE_BANK_ITEM {
        bag_index: 255,
        slot_index: 39,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(store.auto_banked_items.lock().unwrap().as_slice(), &[39]);
}

#[test]
fn autobank_item_err_sends_smsg_inventory_change_failure() {
    // A full destination (bank full, or carry space full) is a per-action error relayed as the
    // existing inventory-change-failure reply, never session-fatal.
    let store = std::sync::Arc::new(InMemoryStore {
        trade_error: Some("bank full".into()),
        ..quest_store()
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_AUTOBANK_ITEM {
        bag_index: 255,
        slot_index: 23,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(_) => {} // correct feedback packet
        other => panic!("expected SMSG_INVENTORY_CHANGE_FAILURE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn autobank_item_from_a_sub_bag_is_unsupported_and_does_not_dispatch() {
    // Only the main pseudo-bag (255) is addressed, matching the item handler's restriction — a
    // sub-bag index is logged and ignored, never fatal.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_AUTOBANK_ITEM {
        bag_index: 19,
        slot_index: 0,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert!(
        store.auto_banked_items.lock().unwrap().is_empty(),
        "a sub-bag source must not be routed through auto_bank_item"
    );
}

#[test]
fn autostore_bank_item_from_a_sub_bag_is_unsupported_and_does_not_dispatch() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_AUTOSTORE_BANK_ITEM {
        bag_index: 19,
        slot_index: 0,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert!(
        store.auto_banked_items.lock().unwrap().is_empty(),
        "a sub-bag source must not be routed through auto_bank_item"
    );
}

#[test]
fn buy_bank_slot_success_sends_ok_and_reaches_the_named_banker() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_BUY_BANK_SLOT {
        guid: Guid::new(88),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_BUY_BANK_SLOT_RESULT(p) => {
            assert_eq!(p.result, BuyBankSlotResult::Ok);
        }
        other => panic!("expected SMSG_BUY_BANK_SLOT_RESULT, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert_eq!(store.bought_bank_slots.lock().unwrap().as_slice(), &[88]);
}

#[test]
fn buy_bank_slot_failure_maps_the_bracketed_code_to_the_matching_result() {
    // The module tags a refusal with its `SMSG_BUY_BANK_SLOT_RESULT` code in brackets — parsed by
    // code, not by matching the prose.
    for (err, want) in [
        (
            "[0] no bank bag slots left to buy",
            BuyBankSlotResult::FailedTooMany,
        ),
        (
            "[1] not enough money (need 1000)",
            BuyBankSlotResult::InsufficientFunds,
        ),
        ("[2] target is not a banker", BuyBankSlotResult::NotBanker),
    ] {
        let mut s = quest_store();
        s.trade_error = Some(err.into());
        let store = std::sync::Arc::new(s);
        let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
        CMSG_BUY_BANK_SLOT {
            guid: Guid::new(88),
        }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
        match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
            ServerOpcodeMessage::SMSG_BUY_BANK_SLOT_RESULT(p) => {
                assert_eq!(p.result, want, "store error {err:?} must map to {want:?}");
            }
            other => panic!("expected SMSG_BUY_BANK_SLOT_RESULT, got {other}"),
        }
        drop(client);
        server.join().unwrap();
    }
}

// ── Inventory change failure ─────────────────────────────────────────────────────

#[test]
fn equip_item_err_sends_smsg_inventory_change_failure() {
    // When `equip_item` returns Err (e.g. item requires higher level / wrong class), the gateway
    // must send SMSG_INVENTORY_CHANGE_FAILURE so the client displays the error sound/popup instead
    // of silently snapping the item back.
    let store = std::sync::Arc::new(InMemoryStore {
        login_entity: Some(warrior_entity()),
        trade_error: Some("required level not met".into()),
        ..tester_store(7)
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    // Slot 24 is a backpack slot (>= 23); bag 255 = INVENTORY_SLOT_BAG_0 (main bag).
    CMSG_AUTOEQUIP_ITEM {
        source_bag: 255,
        source_slot: 24,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(_) => {} // correct feedback packet
        other => panic!("expected SMSG_INVENTORY_CHANGE_FAILURE, got {other}"),
    }
    CMSG_AUTOEQUIP_ITEM {
        source_bag: 255,
        source_slot: 25,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(_) => {}
        other => panic!("expected a second SMSG_INVENTORY_CHANGE_FAILURE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn item_action_before_player_login_is_handled_without_panicking() {
    let store = std::sync::Arc::new(tester_store(7));
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.as_ref()));
    let (mut c_enc, _c_dec) = client_handshake(&mut client, "TESTER", K);

    CMSG_AUTOEQUIP_ITEM {
        source_bag: 255,
        source_slot: 24,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);

    server
        .join()
        .expect("an item action without a selected player must not panic")
        .expect("the legacy zero-actor fallback remains a handled gameplay context");
}

#[test]
fn item_reducer_transport_loss_ends_the_world_session() {
    let store = std::sync::Arc::new(InMemoryStore {
        login_entity: Some(warrior_entity()),
        trade_error: Some("equip_item reducer transport disconnected: channel closed".into()),
        ..tester_store(7)
    });
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        result_tx
            .send(run_world_session(server_end, server_store.as_ref()))
            .unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    for _ in 0..10 {
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    }

    CMSG_AUTOEQUIP_ITEM {
        source_bag: 255,
        source_slot: 24,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();

    let error = result_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("transport loss must end the session promptly")
        .expect_err("a disconnected item reducer transport must be session-fatal");
    assert!(format!("{error:#}").contains("reducer transport disconnected"));
    assert!(
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).is_err(),
        "the socket closes instead of translating transport loss into gameplay feedback"
    );
}

// ── Item-starts-quest ────────────────────────────────────────────────────────────────────────────

#[test]
fn use_item_with_start_quest_opens_details_and_does_not_consume() {
    // Using an item whose template carries `start_quest` must open SMSG_QUESTGIVER_QUEST_DETAILS
    // (the item's OWN instance guid as giver) and must NOT call the normal `use_item` consume path.
    let mut s = quest_store();
    s.item_start_quest_fixture = Some((0x4000_0000_0000_0099, 1234));
    s.quest_details = vec![detail_view(1234, "Report to Goldshire")];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_USE_ITEM {
        bag_index: 255,
        bag_slot: 5,
        spell_index: 0,
        targets: unit_targets(0),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_QUEST_DETAILS(d) => {
            assert_eq!(
                d.guid.guid(),
                0x4000_0000_0000_0099,
                "the item's OWN instance guid is the giver"
            );
            assert_eq!(d.quest_id, 1234);
        }
        other => panic!("expected SMSG_QUESTGIVER_QUEST_DETAILS, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    // The item was NOT consumed — the ordinary use_item path never ran.
    assert!(
        store.used_items.lock().unwrap().is_empty(),
        "start_quest item must not be consumed"
    );
}

#[test]
fn use_item_without_start_quest_falls_through_to_the_ordinary_use_path() {
    // The baseline: an ordinary item (no start_quest fixture) still goes through use_item.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_USE_ITEM {
        bag_index: 255,
        bag_slot: 5,
        spell_index: 0,
        targets: unit_targets(0),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client); // use_item (Ok) sends no SMSG on this path
    server.join().unwrap();
    assert_eq!(store.used_items.lock().unwrap().as_slice(), &[5]);
}

// ── Quest sharing ────────────────────────────────────────────────────────────────────────────────

#[test]
fn push_quest_to_party_dispatches_the_quest_id() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_PUSHQUESTTOPARTY { quest_id: 1234 }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drop(client); // push_quest sends no direct SMSG — the module's group events carry the feedback
    server.join().unwrap();
    assert_eq!(store.pushed_quests.lock().unwrap().as_slice(), &[(7, 1234)]);
}

#[test]
fn push_quest_to_party_rejection_is_logged_and_ignored_not_session_fatal() {
    let mut s = quest_store();
    s.push_quest_error = Some("not in a group".into());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store, 1);
    CMSG_PUSHQUESTTOPARTY { quest_id: 1234 }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drop(client);
    server.join().unwrap(); // must not panic / kill the session on a gameplay rejection
}

// ===========================================================================
// Logout gate tests (blocking logout while in combat)
// ===========================================================================

#[test]
fn logout_while_out_of_combat_succeeds() {
    // combat_until_ms=0 (default, never in combat) → CMSG_LOGOUT_REQUEST must reply
    // Success/Instant + LOGOUT_COMPLETE and the logout() store reducer must be called.
    let store = std::sync::Arc::new(InMemoryStore {
        login_entity: Some(warrior_entity()),
        ..tester_store(7)
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    CMSG_LOGOUT_REQUEST {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    // First message: SMSG_LOGOUT_RESPONSE(Success, Instant)
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LOGOUT_RESPONSE(r) => {
            use wow_world_messages::vanilla::{LogoutResult, LogoutSpeed};
            assert_eq!(
                r.result,
                LogoutResult::Success,
                "expected Success, got {:?}",
                r.result
            );
            assert_eq!(r.speed, LogoutSpeed::Instant);
        }
        other => panic!("expected SMSG_LOGOUT_RESPONSE, got {other}"),
    }
    // Second message: SMSG_LOGOUT_COMPLETE
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LOGOUT_COMPLETE => {}
        other => panic!("expected SMSG_LOGOUT_COMPLETE, got {other}"),
    }

    drop(client);
    server.join().unwrap();

    // The logout() reducer must have been called (entity removal path was taken).
    assert!(
        store
            .logout_called
            .load(std::sync::atomic::Ordering::SeqCst),
        "logout() must be called on a successful out-of-combat logout"
    );
}

#[test]
fn logout_while_in_combat_is_denied() {
    // combat_until_ms=u64::MAX → CMSG_LOGOUT_REQUEST must reply FailureInCombat. The session
    // stays alive (verified by sending a second request and getting a second denial), meaning the
    // entity was NOT removed during the handler — the player cannot escape combat by logging out.
    // Note: socket teardown (drop below) still calls leave_world/logout as cleanup; that is correct
    // and separate from the CMSG gate.
    let store = std::sync::Arc::new(InMemoryStore {
        login_entity: Some(warrior_entity()),
        combat_until_ms: u64::MAX, // always in combat
        ..tester_store(7)
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    // First request: must be denied.
    CMSG_LOGOUT_REQUEST {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LOGOUT_RESPONSE(r) => {
            use wow_world_messages::vanilla::LogoutResult;
            assert_eq!(
                r.result,
                LogoutResult::FailureInCombat,
                "expected FailureInCombat on first request, got {:?}",
                r.result
            );
        }
        other => panic!("expected SMSG_LOGOUT_RESPONSE(denial) on first request, got {other}"),
    }

    // Second request: still denied (session is still alive, entity still in-world).
    // If the first denial had accidentally removed the entity / transitioned to CharSelect,
    // this second request would either hang or produce a different message.
    CMSG_LOGOUT_REQUEST {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LOGOUT_RESPONSE(r) => {
            use wow_world_messages::vanilla::LogoutResult;
            assert_eq!(
                r.result,
                LogoutResult::FailureInCombat,
                "expected FailureInCombat on second request (session still alive), got {:?}",
                r.result
            );
        }
        other => panic!("expected SMSG_LOGOUT_RESPONSE(denial) on second request, got {other}"),
    }

    drop(client);
    server.join().unwrap();
}

#[test]
fn played_time_replies_with_the_durable_total_plus_the_live_session_span() {
    // CMSG_PLAYED_TIME -> SMSG_PLAYED_TIME. The character row carries a durable
    // 3600s total plus a session_start_micros stamped in the recent past, so the reply must be
    // strictly greater than the durable floor (the live span gets folded in) and sane (not absurdly
    // large — bounds the test against a unit mixup, e.g. treating micros as millis).
    let durable_secs: u32 = 3600;
    let session_started_secs_ago: u64 = 5;
    let now_micros = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_micros() as u64;
    let session_start_micros = now_micros - session_started_secs_ago * 1_000_000;

    let store = std::sync::Arc::new(InMemoryStore {
        login_entity: Some(warrior_entity()),
        characters: vec![codec::CharacterView {
            guid: 1,
            name: "Tester".into(),
            played_total_secs: durable_secs,
            session_start_micros,
            ..Default::default()
        }],
        ..tester_store(7)
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    CMSG_PLAYED_TIME {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_PLAYED_TIME(p) => {
            assert!(
                p.total_played_time >= durable_secs,
                "reply {} must be at least the durable floor {durable_secs}",
                p.total_played_time
            );
            assert!(
                p.total_played_time < durable_secs + 60,
                "reply {} should only add a few seconds of live session, not run away",
                p.total_played_time
            );
            assert_eq!(
                p.total_played_time, p.level_played_time,
                "level time mirrors total (untracked per-level in this slice)"
            );
        }
        other => panic!("expected SMSG_PLAYED_TIME, got {other}"),
    }

    drop(client);
    server.join().unwrap();
}

// ===========================================================================================
//  The handler-level tests — CMSG_CAST_SPELL routing, quest instant
//  routing, the loot window state machine, the ATTACKSWING error split, and the smaller mappings.
//  Each drives the full encrypted session (enter_world) and pins wire replies + store dispatches.
// ===========================================================================================

/// Vanilla opcodes for the frames the cast path emits (raw + typed) — pinned as numbers because the
/// synchronous ORDER across `Outbound::Raw` and typed sends is the contract under test.
const OP_CAST_RESULT: u16 = 0x0130;
const OP_SPELL_START: u16 = 0x0131;
const OP_SPELL_GO: u16 = 0x0132;
const OP_LOOT_RESPONSE: u16 = 0x0160;

/// Read one encrypted server frame RAW: decrypt the 4-byte header via the client's `DecrypterHalf`,
/// return `(opcode, body)`. Needed where the wire contract is an `Outbound::Raw` packet (the 5-byte
/// CAST_RESULT ack, the loot/vendor windows) or where the exact opcode ORDER across raw+typed sends
/// is the assertion — gtker's typed reader rejects some of the hand-rolled bodies (it would consume
/// the frame but error), so the bytes are read and pinned directly.
fn read_raw_frame<S: Read>(client: &mut S, dec: &mut DecrypterHalf) -> (u16, Vec<u8>) {
    let h = dec.read_and_decrypt_server_header(&mut *client).unwrap();
    let mut body = vec![0u8; (h.size as usize).saturating_sub(2)];
    client.read_exact(&mut body).unwrap();
    (h.opcode, body)
}

/// `SpellCastTargets` carrying a UNIT target (the client's selected mob).
fn unit_targets(guid: u64) -> SpellCastTargets {
    SpellCastTargets {
        target_flags: SpellCastTargets_SpellCastTargetFlags::new_unit(
            SpellCastTargets_SpellCastTargetFlags_Unit {
                unit_target: Guid::new(guid),
            },
        ),
    }
}

/// `SpellCastTargets` carrying an ITEM target (an enchant cast dropped on a bag item).
fn item_targets(item_guid: u64) -> SpellCastTargets {
    SpellCastTargets {
        target_flags: SpellCastTargets_SpellCastTargetFlags::new_item(
            SpellCastTargets_SpellCastTargetFlags_Item::Item {
                item: Guid::new(item_guid),
            },
        ),
    }
}

/// `SpellCastTargets` carrying a DEST_LOCATION (a ground-targeted click — Flamestrike/Blizzard).
fn dest_targets(x: f32, y: f32, z: f32) -> SpellCastTargets {
    use wow_world_messages::vanilla::{
        SpellCastTargets_SpellCastTargetFlags_DestLocation, Vector3d,
    };
    SpellCastTargets {
        target_flags: SpellCastTargets_SpellCastTargetFlags::new_dest_location(
            SpellCastTargets_SpellCastTargetFlags_DestLocation {
                destination: Vector3d { x, y, z },
            },
        ),
    }
}

// ── CMSG_CAST_SPELL routing (instant-cast ordering, Auto Shot intercept, enchant routing) ──────────

#[test]
fn instant_cast_sends_start_then_raw_cast_result_ok_then_go_and_threads_the_target() {
    // Root-cause client-wedge fix: an INSTANT cast must emit START(0) → raw CAST_RESULT(OK,
    // opcode 0x0130, 5-byte body) → GO synchronously, IN THAT ORDER, and the cast must reach the
    // store with the client's unit target. cast_time_ms = Some(0) is the explicit-instant case.
    let mut s = quest_store();
    s.cast_time_ms = Some(0);
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    CMSG_CAST_SPELL {
        spell: 100,
        targets: unit_targets(77),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();

    let (op1, _) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(op1, OP_SPELL_START, "first frame must be SMSG_SPELL_START");
    let (op2, body2) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(
        op2, OP_CAST_RESULT,
        "second frame must be the raw CAST_RESULT ack"
    );
    let mut want = 100u32.to_le_bytes().to_vec();
    want.push(0x00); // SPELL_RESULT_STATUS_OKAY — 5 bytes, NO trailing reason byte
    assert_eq!(
        body2, want,
        "CAST_RESULT body is spell_id(u32 LE) + OKAY(0x00)"
    );
    let (op3, _) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(op3, OP_SPELL_GO, "third frame must be SMSG_SPELL_GO");

    drop(client);
    server.join().unwrap();
    // The unit target rode CMSG → handler → store unchanged (target-keyed effects need it).
    assert_eq!(store.casts.lock().unwrap().as_slice(), &[(100, 77)]);
}

#[test]
fn unknown_cast_time_is_treated_as_instant() {
    // spell_cast_time = None (spell not in game_spell) → the handler treats the cast as instant
    // (a stray START/GO is harmless; a missing one wedges the client). Default mock = None.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_CAST_SPELL {
        spell: 42,
        targets: SpellCastTargets::default(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (op, _) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(
        op, OP_SPELL_START,
        "unknown cast time must still clear the client synchronously"
    );
    // Drain the rest of the clear (raw CAST_RESULT + GO): closing with unread frames pending
    // resets the socket instead of EOF-ing it, which would end the server side with an error.
    assert_eq!(read_raw_frame(&mut client, &mut c_dec).0, OP_CAST_RESULT);
    assert_eq!(read_raw_frame(&mut client, &mut c_dec).0, OP_SPELL_GO);
    drop(client);
    server.join().unwrap();
    // No unit target in the cast → target 0 (the module substitutes the caster).
    assert_eq!(store.casts.lock().unwrap().as_slice(), &[(42, 0)]);
}

#[test]
fn timed_cast_sends_no_synchronous_start_go_but_still_dispatches() {
    // A TIMED cast keeps the relay path (begin_cast sends START(cast_time); completion sends GO) —
    // the handler must dispatch cast_spell WITHOUT any synchronous START/RESULT/GO of its own.
    let mut s = quest_store();
    s.cast_time_ms = Some(2500);
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_CAST_SPELL {
        spell: 200,
        targets: unit_targets(77),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // Sentinel: the next reply must be the STATUS_QUERY answer — nothing was sent for the cast.
    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {}
        other => panic!(
            "expected SMSG_QUESTGIVER_STATUS (no sync START/GO for a timed cast), got {other}"
        ),
    }
    drop(client);
    server.join().unwrap();
    assert_eq!(store.casts.lock().unwrap().as_slice(), &[(200, 77)]);
}

#[test]
fn ground_targeted_cast_routes_to_cast_spell_at_with_the_click_coords() {
    // 118 phase 2: a cast carrying a DEST_LOCATION target block (the player clicked the ground) must
    // route to cast_spell_at with those coords — NOT the plain cast_spell path — so the module anchors
    // the AoE/patch at the click. Timed (Flamestrike is a 2s cast) → no sync START/GO, same as a normal
    // timed cast; the dest just picks the reducer.
    let mut s = quest_store();
    s.cast_time_ms = Some(2000);
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_CAST_SPELL {
        spell: 2120,
        targets: dest_targets(-9440.0, 64.0, 55.5),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // Sentinel: a timed cast sends nothing synchronously, so the next reply is the STATUS answer.
    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {}
        other => panic!("expected SMSG_QUESTGIVER_STATUS, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    // Landed on cast_spell_at (with coords), and NOT on the plain cast_spell path.
    assert_eq!(
        store.ground_casts.lock().unwrap().as_slice(),
        &[(2120, 0, -9440.0, 64.0, 55.5)]
    );
    assert!(
        store.casts.lock().unwrap().is_empty(),
        "a ground cast must not reach cast_spell"
    );
}

#[test]
fn on_next_swing_cast_sends_no_synchronous_start_go_but_still_dispatches() {
    // 114: an on-next-swing spell (Heroic Strike/Cleave) is INSTANT by cast time, but sends NOTHING
    // synchronously — the client lights the button locally and holds the pending cast; the module's
    // swing-fire cast event (is_completion) later delivers CAST_RESULT(OK)+GO. A sync START/GO here
    // would "resolve" the cast at queue time and un-light the button (the 114 bug).
    let mut s = quest_store();
    s.cast_time_ms = Some(0);
    s.queues_next_swing = true;
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_CAST_SPELL {
        spell: 78,
        targets: unit_targets(77),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // Sentinel: the next reply must be the STATUS_QUERY answer — nothing was sent for the queue cast.
    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {}
        other => panic!("expected SMSG_QUESTGIVER_STATUS (no sync START/GO for an on-next-swing cast), got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert_eq!(store.casts.lock().unwrap().as_slice(), &[(78, 77)]);
}

#[test]
fn rejected_instant_cast_clears_the_client_then_reports_failure() {
    // The synchronous START/RESULT/GO clear goes out BEFORE cast_spell is dispatched; a store
    // rejection then appends SMSG_CAST_RESULT(Failure) — a silent cast-bar reset, not a red error.
    let mut s = quest_store();
    s.cast_spell_error = Some("not enough power: have 0, need 30".into());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_CAST_SPELL {
        spell: 100,
        targets: unit_targets(77),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    assert_eq!(read_raw_frame(&mut client, &mut c_dec).0, OP_SPELL_START);
    assert_eq!(read_raw_frame(&mut client, &mut c_dec).0, OP_CAST_RESULT);
    assert_eq!(read_raw_frame(&mut client, &mut c_dec).0, OP_SPELL_GO);
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CAST_RESULT(r) => {
            assert_eq!(r.spell, 100);
            assert!(matches!(
                r.result,
                SMSG_CAST_RESULT_SimpleSpellCastResult::Failure
            ));
        }
        other => panic!("expected SMSG_CAST_RESULT failure, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn set_sheathed_routes_the_clients_z_press_to_the_store() {
    // #101: CMSG_SETSHEATHED used to reach NO handler — it fell through every arm of `dispatch` and
    // was dropped, so UNIT_FIELD_BYTES_2 stayed 0 forever and peers rendered everyone unarmed.
    // Each of the three real states must reach the store verb with its byte intact.
    for (sent, expect) in [
        (SheathState::Unarmed, 0u8),
        (SheathState::Melee, 1),
        (SheathState::Ranged, 2),
    ] {
        let store = std::sync::Arc::new(quest_store());
        let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
        CMSG_SETSHEATHED { sheathed: sent }
            .write_encrypted_client(&mut client, &mut c_enc)
            .unwrap();
        drop(client);
        server.join().unwrap();
        assert_eq!(
            store.sheathed.lock().unwrap().as_slice(),
            &[(1, expect)],
            "{sent:?} must reach set_sheathed as byte {expect}"
        );
    }
}

#[test]
fn auto_shot_intercept_starts_the_ranged_attack_instead_of_casting() {
    // Vanilla shape: Auto Shot (75) and wand Shoot (5019) are auto-repeat ranged attacks —
    // the handler arms start_ranged_attack with the cast's unit target, then the activation ack is
    // SMSG_SPELL_START ALONE with timer 0 (no CAST_RESULT, no GO — the cast parks in the client's
    // AUTOREPEAT slot and never resolves; each shot's GO comes from the swing-tick relay).
    // cast_spell must NOT run.
    for spell in [75u32, 5019] {
        let store = std::sync::Arc::new(quest_store());
        let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
        CMSG_CAST_SPELL {
            spell,
            targets: unit_targets(88),
        }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
        let (op, body) = read_raw_frame(&mut client, &mut c_dec);
        assert_eq!(
            op, OP_SPELL_START,
            "spell {spell}: activation ack is SPELL_START alone"
        );
        // timer (u32 LE) is the LAST 4 body bytes before... layout: cast_item(packed) caster(packed)
        // spell(4) flags(2) timer(4) targets(..). Cheap pin: the timer bytes right after the u16
        // flags must be 0 — locate spell id then skip flags. spell sits at a packed-guid-dependent
        // offset; both packed self-guids here are 2 bytes (guid 1 -> [0x01, 0x01]).
        let spell_pos = 4; // two 2-byte packed guids
        assert_eq!(
            &body[spell_pos..spell_pos + 4],
            &spell.to_le_bytes(),
            "spell id in START"
        );
        assert_eq!(
            &body[spell_pos + 6..spell_pos + 10],
            &0u32.to_le_bytes(),
            "spell {spell}: START timer must be 0 (the 0.5s wind-up is an attack-timer, not a cast bar)"
        );
        // Nothing else may follow on the activation path (the old phantom GO fired the shoot
        // animation instantly).
        let mut probe = [0u8; 1];
        assert!(
            client.read(&mut probe).map(|n| n == 0).unwrap_or(true),
            "spell {spell}: no packet may follow the activation START"
        );
        drop(client);
        server.join().unwrap();
        assert_eq!(
            store.ranged_attacks.lock().unwrap().as_slice(),
            &[(88, spell)]
        );
        assert!(
            store.casts.lock().unwrap().is_empty(),
            "spell {spell} must not reach cast_spell"
        );
    }
}

#[test]
fn auto_shot_failure_replies_cast_result_only_and_never_arms() {
    // 097 vanilla shape: a rejected activation answers ONLY the raw SMSG_CAST_RESULT(reason) — no
    // SPELL_START (vmangos rejects before sending anything; the 5875 client drops its auto-repeat
    // toggle on the failure result, keeping toggle state in lockstep with the dead server loop).
    let mut s = quest_store();
    s.start_ranged_attack_error = Some("no ranged weapon equipped".into());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_CAST_SPELL {
        spell: 75,
        targets: unit_targets(88),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (op, body) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(
        op, 0x0130,
        "rejection is the raw SMSG_CAST_RESULT, not SPELL_START"
    );
    assert_eq!(&body[0..4], &75u32.to_le_bytes());
    assert_eq!(body[4], 0x02, "status byte = CAST_FAILED");
    drop(client);
    server.join().unwrap();
    assert!(store.ranged_attacks.lock().unwrap().is_empty());
}

#[test]
fn enchant_cast_resolves_the_item_guid_to_its_slot_and_dispatches_the_enchant() {
    // An ITEM-target cast whose spell routes as Enchant(id): item guid → bag slot →
    // enchant_item_on_slot(slot, id), then the manual START → raw CAST_RESULT(OK) → GO clear.
    let mut s = quest_store();
    s.enchant_route = Some(super::EnchantRoute::Enchant(777));
    s.item_slots = vec![(500, 4)];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_CAST_SPELL {
        spell: 7418,
        targets: item_targets(500),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    assert_eq!(read_raw_frame(&mut client, &mut c_dec).0, OP_SPELL_START);
    let (op, body) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(op, OP_CAST_RESULT);
    assert_eq!(body.last(), Some(&0x00), "OK result byte");
    assert_eq!(read_raw_frame(&mut client, &mut c_dec).0, OP_SPELL_GO);
    drop(client);
    server.join().unwrap();
    assert_eq!(store.enchanted.lock().unwrap().as_slice(), &[(4, 777)]);
    assert!(
        store.casts.lock().unwrap().is_empty(),
        "an enchant cast never reaches cast_spell"
    );
}

#[test]
fn disenchant_cast_routes_to_the_disenchant_reducer() {
    let mut s = quest_store();
    s.enchant_route = Some(super::EnchantRoute::Disenchant);
    s.item_slots = vec![(500, 9)];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_CAST_SPELL {
        spell: 13262,
        targets: item_targets(500),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    assert_eq!(read_raw_frame(&mut client, &mut c_dec).0, OP_SPELL_START);
    assert_eq!(read_raw_frame(&mut client, &mut c_dec).0, OP_CAST_RESULT);
    assert_eq!(read_raw_frame(&mut client, &mut c_dec).0, OP_SPELL_GO);
    drop(client);
    server.join().unwrap();
    assert_eq!(store.disenchanted.lock().unwrap().as_slice(), &[9]);
    assert!(store.enchanted.lock().unwrap().is_empty());
}

#[test]
fn enchant_cast_without_an_item_target_replies_failure_and_dispatches_nothing() {
    // An enchant-routed spell with no ITEM target (mis-click) → SMSG_CAST_RESULT(Failure), no
    // START/GO (nothing succeeded to clear), and neither enchant reducer runs.
    let mut s = quest_store();
    s.enchant_route = Some(super::EnchantRoute::Enchant(777));
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_CAST_SPELL {
        spell: 7418,
        targets: SpellCastTargets::default(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CAST_RESULT(r) => {
            assert_eq!(r.spell, 7418);
            assert!(matches!(
                r.result,
                SMSG_CAST_RESULT_SimpleSpellCastResult::Failure
            ));
        }
        other => panic!("expected SMSG_CAST_RESULT failure, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert!(store.enchanted.lock().unwrap().is_empty());
    assert!(store.disenchanted.lock().unwrap().is_empty());
}

// ── Quest instant routing (CMSG_QUESTGIVER_HELLO) ────────────────────────────────────────────────

#[test]
fn quest_hello_with_one_menu_quest_opens_its_screen_directly_by_state() {
    // Vanilla "instant quest": exactly ONE menu-worthy quest skips the list and opens the quest's
    // own screen — DETAILS for a new quest, OFFER_REWARD for a finished turn-in, REQUEST_ITEMS for
    // one still in progress — selected off the giver's END-eval state.
    enum Want {
        Details,
        Offer,
        Request,
    }
    let cases = [
        (eval(1234, codec::ROLE_START, false, false), Want::Details),
        (eval(1234, codec::ROLE_END, true, true), Want::Offer),
        (eval(1234, codec::ROLE_END, true, false), Want::Request),
    ];
    for (e, want) in cases {
        let mut s = quest_store();
        s.quest_evals = vec![e];
        s.quest_details = vec![detail_view(1234, "A Threat Within")];
        let store = std::sync::Arc::new(s);
        let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
        CMSG_QUESTGIVER_HELLO {
            guid: Guid::new(50),
        }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
        match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
            ServerOpcodeMessage::SMSG_QUESTGIVER_QUEST_DETAILS(_) => {
                assert!(
                    matches!(want, Want::Details),
                    "got DETAILS for a turn-in state"
                )
            }
            ServerOpcodeMessage::SMSG_QUESTGIVER_OFFER_REWARD(_) => {
                assert!(
                    matches!(want, Want::Offer),
                    "got OFFER_REWARD but the quest isn't complete"
                )
            }
            ServerOpcodeMessage::SMSG_QUESTGIVER_REQUEST_ITEMS(_) => {
                assert!(
                    matches!(want, Want::Request),
                    "got REQUEST_ITEMS but the quest is complete"
                )
            }
            other => panic!("expected a direct quest screen, got {other}"),
        }
        drop(client);
        server.join().unwrap();
    }
}

#[test]
fn quest_hello_with_two_menu_quests_shows_the_list() {
    // ≥2 menu-worthy quests → the list window, even though both details are loaded.
    let mut s = quest_store();
    s.quest_evals = vec![
        eval(1234, codec::ROLE_START, false, false),
        eval(1235, codec::ROLE_START, false, false),
    ];
    s.quest_details = vec![detail_view(1234, "One"), detail_view(1235, "Two")];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_QUESTGIVER_HELLO {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_QUEST_LIST(l) => {
            assert_eq!(l.quest_items.len(), 2, "both quests listed");
        }
        other => panic!("expected SMSG_QUESTGIVER_QUEST_LIST, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

// ── Loot window state machine ────────────────────────────────────────────────────────────────────

#[test]
fn loot_opens_the_window_and_loot_money_drives_the_tracked_guid() {
    // CMSG_LOOT arms looting_target and replies the RAW loot window (guid + money in the body);
    // CMSG_LOOT_MONEY (which carries NO guid) must then hit the TRACKED corpse. A
    // SOLO money loot sends ONLY SMSG_LOOT_CLEAR_MONEY — the unconditional SMSG_LOOT_MONEY_NOTIFY
    // is gone (vanilla never sends it to a solo looter; the client prints its own local "You loot X
    // copper" line). A corpse with money is NOT skinned.
    let mut s = quest_store();
    s.corpse_money = 25;
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    CMSG_LOOT {
        guid: Guid::new(60),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (op, body) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(op, OP_LOOT_RESPONSE);
    assert_eq!(
        &body[0..8],
        &60u64.to_le_bytes(),
        "loot window names the corpse guid"
    );
    assert_eq!(
        &body[9..13],
        &25u32.to_le_bytes(),
        "loot window shows the corpse's copper"
    );

    CMSG_LOOT_MONEY {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LOOT_CLEAR_MONEY => {}
        other => panic!(
            "expected SMSG_LOOT_CLEAR_MONEY directly (no notify for a solo looter), got {other}"
        ),
    }
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.money_looted.lock().unwrap().as_slice(),
        &[60],
        "the TRACKED guid was looted"
    );
    assert!(
        store.skinned.lock().unwrap().is_empty(),
        "a corpse with money is not skinned"
    );
}

#[test]
fn loot_money_with_zero_copper_still_clears_with_no_notify() {
    // amount == 0: the same no-notify contract as any solo loot — CLEAR_MONEY still
    // goes out so the client's loot window drops its money row.
    let store = std::sync::Arc::new(quest_store()); // corpse_money = 0
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_LOOT {
        guid: Guid::new(60),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let _ = read_raw_frame(&mut client, &mut c_dec); // the loot window
    CMSG_LOOT_MONEY {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LOOT_CLEAR_MONEY => {} // and NOT a NOTIFY first
        other => {
            panic!("expected SMSG_LOOT_CLEAR_MONEY directly (no notify for 0 copper), got {other}")
        }
    }
    drop(client);
    server.join().unwrap();
    assert_eq!(store.money_looted.lock().unwrap().as_slice(), &[60]);
}

#[test]
fn looting_a_fully_emptied_corpse_attempts_the_skinning_fallback() {
    // Skin fallback fires only when the loot is EMPTY: no items AND no money (the mock's defaults).
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_LOOT {
        guid: Guid::new(61),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (op, _) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(
        op, OP_LOOT_RESPONSE,
        "the empty window still opens (safe fallback feedback)"
    );
    drop(client);
    server.join().unwrap();
    assert_eq!(store.skinned.lock().unwrap().as_slice(), &[61]);
}

#[test]
fn loot_release_clears_the_tracked_target_so_loot_money_is_a_noop() {
    let mut s = quest_store();
    s.corpse_money = 25; // non-empty so the skin fallback stays out of the picture
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_LOOT {
        guid: Guid::new(60),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let _ = read_raw_frame(&mut client, &mut c_dec);
    CMSG_LOOT_RELEASE {
        guid: Guid::new(60),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LOOT_RELEASE_RESPONSE(r) => assert_eq!(r.guid.guid(), 60),
        other => panic!("expected SMSG_LOOT_RELEASE_RESPONSE, got {other}"),
    }
    // The window is closed — a stray CMSG_LOOT_MONEY must not reach the store.
    CMSG_LOOT_MONEY {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drop(client);
    server.join().unwrap();
    assert!(
        store.money_looted.lock().unwrap().is_empty(),
        "release cleared the tracked target"
    );
}

// ── Group loot methods ───────────────────────────────────────────────────────────────────────────

#[test]
fn loot_method_dispatches_the_decoded_setting_threshold_and_master() {
    // CMSG_LOOT_METHOD (leader sets MASTER LOOT, Epic threshold, master guid 7) must reach the
    // store with the gateway-decoded wire bytes — a direct pass-through (module adopted the wire
    // ordering verbatim), no separate ack packet (vanilla sends none for this opcode either).
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_LOOT_METHOD {
        loot_setting: GroupLootSetting::MasterLoot,
        loot_master: Guid::new(7),
        loot_threshold: ItemQuality::Epic,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // No reply packet for this opcode — send a harmless follow-up (CMSG_LOOT_MONEY, a no-op here
    // with no tracked target) and confirm it's the NEXT thing the server processes, proving the
    // method call didn't hang the dispatch loop waiting to send something.
    CMSG_LOOT_MONEY {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.group_loot_methods.lock().unwrap().as_slice(),
        &[(
            GroupLootSetting::MasterLoot.as_int(),
            7,
            ItemQuality::Epic.as_int()
        )]
    );
}

#[test]
fn loot_roll_dispatches_the_corpse_slot_and_vote() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_LOOT_ROLL {
        item: Guid::new(60),
        item_slot: 2,
        vote: RollVote::Need,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_LOOT_MONEY {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.loot_rolls.lock().unwrap().as_slice(),
        &[(60, 2, RollVote::Need.as_int())]
    );
}

#[test]
fn loot_master_give_dispatches_the_corpse_slot_and_target() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_LOOT_MASTER_GIVE {
        loot: Guid::new(60),
        slot_id: 3,
        player: Guid::new(9),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_LOOT_MONEY {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.loot_master_gives.lock().unwrap().as_slice(),
        &[(60, 3, 9)]
    );
}

#[test]
fn loot_roll_rejection_is_logged_and_ignored_not_session_fatal() {
    // A rejection (no roll open / already voted / not eligible) must not tear the connection down —
    // the SAME session keeps working afterward (mirrors take_loot's per-action ignore discipline).
    let mut s = quest_store();
    s.trade_error = Some("no roll open on that item".to_string());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_LOOT_ROLL {
        item: Guid::new(60),
        item_slot: 2,
        vote: RollVote::Greed,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // The session survives: a subsequent CMSG_LOOT still gets a normal reply.
    CMSG_LOOT {
        guid: Guid::new(61),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (op, _) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(
        op, OP_LOOT_RESPONSE,
        "the session must survive a rejected loot_roll"
    );
    drop(client);
    server.join().unwrap();
}

// ── Per-viewer quest loot ────────────────────────────────────────────────────────────────────────
// `corpse_loot` now takes a VIEWER guid; these tests pin the WIRING — that `CMSG_LOOT`/
// `CMSG_GAMEOBJ_USE` thread `iw.self_guid` through to the store call so two different viewers of the
// same corpse can be served two different windows. The actual quest-need FILTER decision
// (`quest_row_visible_to_viewer`) is unit-tested directly in `gateway/src/stdb/reads.rs` — this
// `InMemoryStore` fixture stands in for whatever that real filter would have produced per viewer.

fn loot_item_bytes(body: &[u8], index: usize) -> (u8, u32, u32, u32) {
    // Item N starts at byte 14 (8 guid + 1 method + 4 money + 1 count), 22 bytes each.
    let base = 14 + index * 22;
    let slot = body[base];
    let item_id = u32::from_le_bytes(body[base + 1..base + 5].try_into().unwrap());
    let count = u32::from_le_bytes(body[base + 5..base + 9].try_into().unwrap());
    let display_id = u32::from_le_bytes(body[base + 9..base + 13].try_into().unwrap());
    (slot, item_id, count, display_id)
}

#[test]
fn corpse_loot_is_threaded_with_the_viewers_own_guid_not_the_corpses() {
    // A quest item only in viewer 1's fixture, a DIFFERENT (non-quest) item only in viewer 2's — same
    // corpse guid (60), two separate connections sharing one store. Each must see ONLY their own row:
    // proves `iw.self_guid` (not some corpse-keyed or global lookup) drives the `corpse_loot` call.
    let mut s = quest_store();
    s.corpse_loot_by_viewer
        .insert(1, vec![(0u8, 6948, 1u32, 100u32)]); // viewer 1: quest item
    s.corpse_loot_by_viewer
        .insert(2, vec![(0u8, 2589, 5u32, 200u32)]); // viewer 2: a different item
    let store = std::sync::Arc::new(s);

    let (mut c1, mut e1, mut d1, s1) = enter_world(store.clone(), 1);
    CMSG_LOOT {
        guid: Guid::new(60),
    }
    .write_encrypted_client(&mut c1, &mut e1)
    .unwrap();
    let (op1, body1) = read_raw_frame(&mut c1, &mut d1);
    assert_eq!(op1, OP_LOOT_RESPONSE);
    assert_eq!(body1[13], 1, "viewer 1 sees exactly their one row");
    assert_eq!(
        loot_item_bytes(&body1, 0),
        (0, 6948, 1, 100),
        "viewer 1's own item, not viewer 2's"
    );
    drop(c1);
    s1.join().unwrap();

    let (mut c2, mut e2, mut d2, s2) = enter_world(store.clone(), 2);
    CMSG_LOOT {
        guid: Guid::new(60),
    }
    .write_encrypted_client(&mut c2, &mut e2)
    .unwrap();
    let (op2, body2) = read_raw_frame(&mut c2, &mut d2);
    assert_eq!(op2, OP_LOOT_RESPONSE);
    assert_eq!(body2[13], 1, "viewer 2 sees exactly their one row");
    assert_eq!(
        loot_item_bytes(&body2, 0),
        (0, 2589, 5, 200),
        "viewer 2's own item, not viewer 1's"
    );
    drop(c2);
    s2.join().unwrap();
}

#[test]
fn both_grouped_viewers_see_their_own_row_when_both_need_the_quest_item() {
    // "Both have it -> both loot one each": the SAME shared corpse,
    // BOTH viewers' fixtures carry a row (simulating the real filter admitting the row to each because
    // each independently needs it) — each connection's window shows its own row unaffected by the
    // other's presence.
    let mut s = quest_store();
    s.corpse_loot_by_viewer
        .insert(1, vec![(0u8, 6948, 1u32, 100u32)]);
    s.corpse_loot_by_viewer
        .insert(2, vec![(0u8, 6948, 1u32, 100u32)]); // same item, viewer 2's own row
    let store = std::sync::Arc::new(s);

    for viewer in [1u64, 2u64] {
        let (mut c, mut e, mut d, srv) = enter_world(store.clone(), viewer);
        CMSG_LOOT {
            guid: Guid::new(60),
        }
        .write_encrypted_client(&mut c, &mut e)
        .unwrap();
        let (op, body) = read_raw_frame(&mut c, &mut d);
        assert_eq!(op, OP_LOOT_RESPONSE);
        assert_eq!(
            body[13], 1,
            "viewer {viewer} sees their own copy of the quest row"
        );
        assert_eq!(
            loot_item_bytes(&body, 0).1,
            6948,
            "viewer {viewer}'s own quest item"
        );
        drop(c);
        srv.join().unwrap();
    }
}

#[test]
fn a_viewer_with_no_fixture_entry_sees_an_empty_window_non_quest_rows_unaffected() {
    // A viewer nobody set up a fixture for (e.g. doesn't need the quest, or the corpse has no
    // quest-only rows at all — the common case) sees an empty window, exactly the existing
    // default behavior this slice must not disturb.
    let store = std::sync::Arc::new(quest_store()); // corpse_loot_by_viewer empty
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_LOOT {
        guid: Guid::new(60),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (op, body) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(op, OP_LOOT_RESPONSE);
    assert_eq!(
        body[13], 0,
        "no fixture for this viewer -> zero items, same as before 187"
    );
    drop(client);
    server.join().unwrap();
}

// ── CMSG_ATTACKSWING error split + happy path (combat C1) ───────────────────────────────────────

#[test]
fn attackswing_ok_replies_attackstart_and_stop_echoes_then_clears() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_ATTACKSWING {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_ATTACKSTART(a) => {
            assert_eq!(a.attacker.guid(), 1, "self guid");
            assert_eq!(a.victim.guid(), 90);
        }
        other => panic!("expected SMSG_ATTACKSTART, got {other}"),
    }
    // Stop echoes the armed target and clears it.
    CMSG_ATTACKSTOP {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_ATTACKSTOP(a) => {
            assert_eq!(a.player.guid(), 1);
            assert_eq!(a.enemy.guid(), 90);
        }
        other => panic!("expected SMSG_ATTACKSTOP, got {other}"),
    }
    // A second stop finds no armed target → NO second echo (sentinel replies first).
    CMSG_ATTACKSTOP {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {}
        other => panic!("expected the sentinel (no ATTACKSTOP echo once cleared), got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn attackswing_at_a_dead_target_replies_deadtarget() {
    let mut s = quest_store();
    s.start_attack_error = Some(lyracore_shared::ERR_ATTACK_TARGET_DEAD.into());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_ATTACKSWING {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_ATTACKSWING_DEADTARGET => {}
        other => panic!("expected SMSG_ATTACKSWING_DEADTARGET, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn attackswing_at_a_friendly_target_replies_cant_attack() {
    let mut s = quest_store();
    s.start_attack_error = Some(lyracore_shared::ERR_ATTACK_FRIENDLY.into());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_ATTACKSWING {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_ATTACKSWING_CANT_ATTACK => {}
        other => panic!("expected SMSG_ATTACKSWING_CANT_ATTACK, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn attackswing_desync_error_is_session_fatal() {
    // A desync-classified start_attack failure (the player's OWN entity is gone) must PROPAGATE
    // as session-fatal: run_world_session returns Err and the socket tears down for a clean relog —
    // unlike the transient dead/friendly failures above, which keep the session alive.
    let mut s = quest_store();
    s.start_attack_error = Some("no live entity for guid 1".into());
    let store = std::sync::Arc::new(s);
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    // Roll enter_world by hand: the server thread must RETURN the session result (not unwrap it).
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.as_ref()));
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    for _ in 0..10 {
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    }
    CMSG_ATTACKSWING {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let result = server.join().unwrap();
    let err = result.expect_err("a desync on attackswing must end the session with an error");
    assert!(
        format!("{err:#}").contains("desync"),
        "the error should carry the desync context, got: {err:#}"
    );
    drop(client);
}

// ── Smaller mappings: WHO, buyback slots, trainer buy, talents, gossip select, chat, epochs ─────

#[test]
fn who_reply_lists_every_online_player_with_level_and_zone() {
    let mut s = quest_store();
    s.characters = vec![
        codec::CharacterView {
            guid: 2,
            name: "Alpha".into(),
            race: 1,
            class: 1,
            level: 5,
            zone_id: 12,
            ..Default::default()
        },
        codec::CharacterView {
            guid: 3,
            name: "Bravo".into(),
            race: 1,
            class: 1,
            level: 60,
            zone_id: 12,
            ..Default::default()
        },
    ];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_WHO {
        minimum_level: Level::new(1),
        maximum_level: Level::new(60),
        player_name: String::new(),
        guild_name: String::new(),
        race_mask: 0,
        class_mask: 0,
        zones: Vec::new(),
        search_strings: Vec::new(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_WHO(w) => {
            assert_eq!(w.online_players, 2);
            assert_eq!(w.players.len(), 2);
            assert_eq!(w.players[0].name, "Alpha");
            assert_eq!(w.players[0].level, Level::new(5));
            assert_eq!(w.players[1].name, "Bravo");
            assert_eq!(w.players[1].level, Level::new(60));
        }
        other => panic!("expected SMSG_WHO, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn buyback_maps_the_wire_slot_enum_to_zero_based_ring_slots() {
    // BuybackSlot rides as 69..=81 on the wire; the store reducer takes 0-based ring slots —
    // Slot1 (69) → 0, Slot13 (81) → 12.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_BUYBACK_ITEM {
        guid: Guid::new(99),
        slot: BuybackSlot::Slot1,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_BUYBACK_ITEM {
        guid: Guid::new(99),
        slot: BuybackSlot::Slot13,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // 248: a successful buyback now pushes the refreshed tab view (one raw VALUES per call —
    // the mock ring is empty, so no item CREATEs). Consume both frames before EOF; gtker cannot
    // DECODE a hand-rolled partial VALUES mask (no OBJECT_FIELD_TYPE — the raw path's whole
    // reason to exist), so tolerate the parse error: the frame bytes are consumed either way.
    let _ = ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec);
    let _ = ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec);
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.bought_back.lock().unwrap().as_slice(),
        &[(99, 0), (99, 12)]
    );
}

#[test]
fn trainer_buy_success_replies_succeeded_then_pushes_the_learned_spell() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_TRAINER_BUY_SPELL {
        guid: Guid::new(70),
        id: 1234,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_TRAINER_BUY_SUCCEEDED(m) => {
            assert_eq!(m.guid.guid(), 70);
            assert_eq!(m.id, 1234);
        }
        other => panic!("expected SMSG_TRAINER_BUY_SUCCEEDED, got {other}"),
    }
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LEARNED_SPELL(m) => assert_eq!(m.id, 1234),
        other => panic!("expected SMSG_LEARNED_SPELL, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn trainer_buy_failure_parses_the_reason_tag_into_the_failure_code() {
    // The module tags its Err with gtker's `[N]` reason: [1]=money, [2]=level/req, else generic.
    for (err, want) in [
        ("too poor [1]", TrainingFailureReason::NotEnoughMoney),
        ("level too low [2]", TrainingFailureReason::NotEnoughSkill),
        ("some other rejection", TrainingFailureReason::Unavailable),
    ] {
        let mut s = quest_store();
        s.trade_error = Some(err.into());
        let store = std::sync::Arc::new(s);
        let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
        CMSG_TRAINER_BUY_SPELL {
            guid: Guid::new(70),
            id: 1234,
        }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
        match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
            ServerOpcodeMessage::SMSG_TRAINER_BUY_FAILED(m) => {
                assert_eq!(m.error, want, "store error {err:?} must map to {want:?}");
                assert_eq!(m.id, 1234);
            }
            other => panic!("expected SMSG_TRAINER_BUY_FAILED, got {other}"),
        }
        drop(client);
        server.join().unwrap();
    }
}

#[test]
fn fishing_cast_routes_to_the_fish_reducer_with_the_manual_clear() {
    // 060: a spell flagged E_FISH routes CMSG_CAST_SPELL to the fish reducer (never cast_spell),
    // acked with the manual START -> raw CAST_RESULT(OK) -> GO clear (the enchant shape).
    let mut s = quest_store();
    s.fishing_spells = vec![7620];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_CAST_SPELL {
        spell: 7620,
        targets: SpellCastTargets::default(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    assert_eq!(read_raw_frame(&mut client, &mut c_dec).0, OP_SPELL_START);
    assert_eq!(
        read_raw_frame(&mut client, &mut c_dec).0,
        0x0130,
        "raw CAST_RESULT(OK)"
    );
    assert_eq!(read_raw_frame(&mut client, &mut c_dec).0, OP_SPELL_GO);
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.fish_casts.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert!(
        store.casts.lock().unwrap().is_empty(),
        "fishing must not reach cast_spell"
    );
}

#[test]
fn pick_lock_cast_routes_to_the_pick_lock_reducer_with_the_manual_clear() {
    // 119: a spell flagged E_OPEN_LOCK routes CMSG_CAST_SPELL to the pick_lock reducer (never
    // cast_spell), with the target GO guid decoded off the cast's GAMEOBJECT target block, and acked
    // with the manual START -> raw CAST_RESULT(OK) -> GO clear (the enchant/fish shape).
    use wow_world_messages::vanilla::SpellCastTargets_SpellCastTargetFlags_Gameobject as GoTgt;
    let mut s = quest_store();
    s.open_lock_spells = vec![1804];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    let targets = SpellCastTargets {
        target_flags:
            wow_world_messages::vanilla::SpellCastTargets_SpellCastTargetFlags::new_gameobject(
                GoTgt::Gameobject {
                    gameobject: Guid::new(0xABCD),
                },
            ),
    };
    CMSG_CAST_SPELL {
        spell: 1804,
        targets,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    assert_eq!(read_raw_frame(&mut client, &mut c_dec).0, OP_SPELL_START);
    assert_eq!(
        read_raw_frame(&mut client, &mut c_dec).0,
        0x0130,
        "raw CAST_RESULT(OK)"
    );
    assert_eq!(read_raw_frame(&mut client, &mut c_dec).0, OP_SPELL_GO);
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.pick_lock_casts.lock().unwrap().as_slice(),
        &[0xABCD],
        "pick_lock got the GO guid off the cast"
    );
    assert!(
        store.casts.lock().unwrap().is_empty(),
        "pick lock must not reach cast_spell"
    );
}

#[test]
fn learn_talent_with_a_grant_spell_pushes_learned_spell() {
    // An ability talent (grant_spell_id != 0) + a successful learn → SMSG_LEARNED_SPELL(grant) so
    // the new button is usable without a relog.
    let mut s = quest_store();
    s.talent_grant = 2098;
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_LEARN_TALENT {
        talent: Talent::BurningSoul,
        requested_rank: 0,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LEARNED_SPELL(m) => assert_eq!(m.id, 2098),
        other => panic!("expected SMSG_LEARNED_SPELL, got {other}"),
    }
    // The CHARACTER_POINTS1 VALUES push follows (raw read — the dirty_reset partial deliberately
    // omits OBJECT_FIELD_TYPE, which gtker's typed reader refuses).
    assert_eq!(read_raw_frame(&mut client, &mut c_dec).0, 0x00A9);
    drop(client);
    server.join().unwrap();
}

#[test]
fn learn_talent_passive_pushes_rank_spell_and_points() {
    // A PASSIVE pick must still refresh the 1.12 TalentFrame live: SMSG_LEARNED_SPELL for the
    // RANK-SPELL the module taught (the pane derives shown ranks from known rank-spells —
    // SPELLS_CHANGED) followed by the PLAYER_CHARACTER_POINTS1 partial VALUES
    // (CHARACTER_POINTS_CHANGED). The old behavior sent NOTHING → pane frozen until relog.
    let mut s = quest_store(); // talent_grant = 0
    s.talent_pane = (7777, 0, 2); // rank-spell 7777 taught, no superseded prev, 2 points left
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_LEARN_TALENT {
        talent: Talent::BurningSoul,
        requested_rank: 0,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LEARNED_SPELL(m) => assert_eq!(m.id, 7777),
        other => panic!("expected SMSG_LEARNED_SPELL(rank-spell), got {other}"),
    }
    // Raw read: the dirty_reset partial VALUES omits OBJECT_FIELD_TYPE (gtker's typed reader refuses).
    assert_eq!(
        read_raw_frame(&mut client, &mut c_dec).0,
        0x00A9,
        "the CHARACTER_POINTS1 VALUES push"
    );
    drop(client);
    server.join().unwrap();
}

#[test]
fn learn_talent_rank_upgrade_supersedes_the_previous_rank_spell() {
    // Rank N>1: the previous rank's spell is REPLACED in the book — SMSG_SUPERCEDED_SPELL with the
    // cmangos wire order (OLD rides the first u16 slot), mirroring the trainer rank-upgrade path.
    let mut s = quest_store();
    s.talent_pane = (7778, 7777, 1); // new rank-spell 7778 supersedes 7777, 1 point left
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_LEARN_TALENT {
        talent: Talent::BurningSoul,
        requested_rank: 0,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_SUPERCEDED_SPELL(m) => {
            assert_eq!(
                m.new_spell_id, 7777,
                "first wire slot carries the OLD rank (cmangos order)"
            );
            assert_eq!(
                m.old_spell_id, 7778,
                "second wire slot carries the NEW rank"
            );
        }
        other => panic!("expected SMSG_SUPERCEDED_SPELL, got {other}"),
    }
    assert_eq!(
        read_raw_frame(&mut client, &mut c_dec).0,
        0x00A9,
        "the CHARACTER_POINTS1 VALUES push"
    );
    drop(client);
    server.join().unwrap();
}

#[test]
fn list_inventory_opens_the_vendor_window_over_the_socket() {
    let mut s = quest_store();
    s.vendor_stock = vec![codec::VendorItemView {
        item_entry: 4540,
        display_id: 6353,
        buy_price: 25,
        ..Default::default()
    }];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_LIST_INVENTORY {
        guid: Guid::new(80),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (op, body) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(op, codec::SMSG_LIST_INVENTORY_OPCODE);
    assert_eq!(&body[0..8], &80u64.to_le_bytes());
    assert_eq!(body[8], 1, "one stocked item");
    drop(client);
    server.join().unwrap();
}

#[test]
fn list_inventory_on_a_standing_refusing_vendor_sends_no_reply() {
    let store = std::sync::Arc::new(InMemoryStore {
        npc_refuses: true,
        characters: vec![codec::CharacterView {
            guid: 1,
            ..Default::default()
        }],
        ..quest_store()
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_LIST_INVENTORY {
        guid: Guid::new(80),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // Sentinel: a follow-up request with a guaranteed reply — a wrongly sent vendor window
    // would arrive ahead of it.
    CMSG_PLAYED_TIME {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_PLAYED_TIME(_) => {}
        other => panic!("expected SMSG_PLAYED_TIME (refused vendor answers nothing), got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn gossip_select_on_a_vendor_opens_the_inventory_window() {
    // Option 0 on a stocked NPC is "browse goods" → the RAW SMSG_LIST_INVENTORY, same as the
    // direct CMSG_LIST_INVENTORY path.
    let mut s = quest_store();
    s.vendor_stock = vec![codec::VendorItemView {
        item_entry: 4540,
        display_id: 6353,
        buy_price: 25,
        ..Default::default()
    }];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    let menu = gossip_hello(&mut client, &mut c_enc, &mut c_dec, 80);
    assert_eq!(menu.gossips[0].message, "I'd like to browse your goods.");
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(80),
        gossip_list_id: 0,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (op, body) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(op, codec::SMSG_LIST_INVENTORY_OPCODE);
    assert_eq!(
        &body[0..8],
        &80u64.to_le_bytes(),
        "the vendor window names the NPC"
    );
    drop(client);
    server.join().unwrap();
    assert!(!store.home_bound.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn gossip_select_on_an_innkeeper_binds_home_and_completes() {
    // A non-vendor innkeeper's "Make this inn your home." is option 0 → bind_home + GOSSIP_COMPLETE.
    let mut s = quest_store();
    s.innkeeper = true;
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    let menu = gossip_hello(&mut client, &mut c_enc, &mut c_dec, 81);
    assert_eq!(menu.gossips[0].message, "Make this inn your home.");
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(81),
        gossip_list_id: 0,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE => {}
        other => panic!("expected SMSG_GOSSIP_COMPLETE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert!(
        store.home_bound.load(std::sync::atomic::Ordering::SeqCst),
        "bind_home must have run"
    );
}

#[test]
fn gossip_select_of_any_other_option_completes_without_binding() {
    // Farewell (option 1 on an innkeeper NPC) → GOSSIP_COMPLETE only; no bind, no vendor window.
    let mut s = quest_store();
    s.innkeeper = true;
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    gossip_hello(&mut client, &mut c_enc, &mut c_dec, 81);
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(81),
        gossip_list_id: 1,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE => {}
        other => panic!("expected SMSG_GOSSIP_COMPLETE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert!(!store.home_bound.load(std::sync::atomic::Ordering::SeqCst));
}

// --- Imported gossip menu options + multi-slot npc_text -------------------------------------------

/// Open `npc`'s gossip window and drain the menu, so a following `CMSG_GOSSIP_SELECT_OPTION` has the
/// snapshot it resolves against — a click with nothing open selects nothing.
fn gossip_hello(
    client: &mut UnixStream,
    enc: &mut EncrypterHalf,
    dec: &mut DecrypterHalf,
    npc: u64,
) -> wow_world_messages::vanilla::SMSG_GOSSIP_MESSAGE {
    CMSG_GOSSIP_HELLO {
        guid: Guid::new(npc),
    }
    .write_encrypted_client(&mut *client, enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut *client, dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(m) => *m,
        other => panic!("expected SMSG_GOSSIP_MESSAGE, got {other}"),
    }
}

/// A shorthand imported option builder for the gossip mock tests.
fn opt(icon: u32, text: &str, action: u32) -> codec::GossipOptionView {
    codec::GossipOptionView {
        icon,
        text: text.to_string(),
        action,
        ..Default::default()
    }
}

#[test]
fn gossip_hello_renders_imported_options_verbatim_with_a_trailing_farewell() {
    // The 217 acceptance criterion: an Elwynn-innkeeper-shaped NPC with 3 imported options (chat,
    // browse goods, make-home) renders them VERBATIM (real dump text, not the hardcoded fallback
    // strings) — the vendor/innkeeper flags are ignored entirely once options are imported.
    use lyracore_shared::constants::gossip_option;
    let mut s = quest_store();
    s.gossip_opts = vec![
        opt(0, "Well met, traveler.", gossip_option::GOSSIP),
        opt(1, "I'd like to browse your goods.", gossip_option::VENDOR),
        opt(
            0,
            "I'd like to stay here a while.",
            gossip_option::INNKEEPER,
        ),
    ];
    // Fallback signals present too — must be ignored while options are imported.
    s.vendor_stock = vec![codec::VendorItemView {
        item_entry: 1,
        ..Default::default()
    }];
    s.innkeeper = true;
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_GOSSIP_HELLO {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(m) => {
            assert_eq!(
                m.gossips.len(),
                4,
                "3 imported + a trailing Farewell: {:?}",
                m.gossips
            );
            assert_eq!(m.gossips[0].message, "Well met, traveler.");
            assert_eq!(m.gossips[1].message, "I'd like to browse your goods.");
            assert_eq!(m.gossips[2].message, "I'd like to stay here a while.");
            assert_eq!(m.gossips[3].message, "Farewell.");
        }
        other => panic!("expected SMSG_GOSSIP_MESSAGE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn gossip_select_on_an_imported_vendor_option_opens_the_inventory_window() {
    use lyracore_shared::constants::gossip_option;
    let mut s = quest_store();
    s.gossip_opts = vec![opt(1, "Browse.", gossip_option::VENDOR)];
    s.vendor_stock = vec![codec::VendorItemView {
        item_entry: 4540,
        display_id: 6353,
        buy_price: 25,
        ..Default::default()
    }];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    gossip_hello(&mut client, &mut c_enc, &mut c_dec, 90);
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(90),
        gossip_list_id: 0,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (op, body) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(op, codec::SMSG_LIST_INVENTORY_OPCODE);
    assert_eq!(
        &body[0..8],
        &90u64.to_le_bytes(),
        "the vendor window names the NPC"
    );
    drop(client);
    server.join().unwrap();
    assert!(!store.home_bound.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn gossip_select_on_an_imported_innkeeper_option_binds_home() {
    use lyracore_shared::constants::gossip_option;
    let mut s = quest_store();
    s.gossip_opts = vec![
        opt(0, "Chat.", gossip_option::GOSSIP),
        opt(0, "Stay here.", gossip_option::INNKEEPER),
    ];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    gossip_hello(&mut client, &mut c_enc, &mut c_dec, 90);
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(90),
        gossip_list_id: 1,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE => {}
        other => panic!("expected SMSG_GOSSIP_COMPLETE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert!(
        store.home_bound.load(std::sync::atomic::Ordering::SeqCst),
        "bind_home must have run"
    );
}

#[test]
fn the_same_option_row_reaches_the_module_by_row_id_from_either_viewer() {
    use lyracore_shared::constants::{gossip_condition, gossip_option};
    let menu = || {
        let mut gated = opt(0, "About that favor...", gossip_option::GOSSIP);
        gated.row_id = 4001;
        gated.cond_type = gossip_condition::QUEST_TAKEN;
        gated.cond_value1 = 60;
        let mut always = opt(0, "Stay here.", gossip_option::INNKEEPER);
        always.row_id = 4002;
        vec![gated, always]
    };
    // Viewer A has not taken quest 60 → the gated row is hidden, so "Stay here." renders at 0.
    let mut a = quest_store();
    a.gossip_opts = menu();
    let a = std::sync::Arc::new(a);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(a.clone(), 1);
    let rendered = gossip_hello(&mut client, &mut c_enc, &mut c_dec, 90);
    assert_eq!(rendered.gossips[0].message, "Stay here.");
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(90),
        gossip_list_id: 0,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let _ = ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    drop(client);
    server.join().unwrap();

    // Viewer B HAS taken it → the gated row renders first and pushes "Stay here." to position 1.
    let mut b = quest_store();
    b.gossip_opts = menu();
    b.quest_log = vec![(60, false)].into();
    let b = std::sync::Arc::new(b);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(b.clone(), 1);
    let rendered = gossip_hello(&mut client, &mut c_enc, &mut c_dec, 90);
    assert_eq!(rendered.gossips[1].message, "Stay here.");
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(90),
        gossip_list_id: 1,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let _ = ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    drop(client);
    server.join().unwrap();

    let (a_pos, a_row) = a.gossip_selects.lock().unwrap()[0];
    let (b_pos, b_row) = b.gossip_selects.lock().unwrap()[0];
    assert_ne!(a_pos, b_pos, "the POSITION differs between the two viewers");
    assert_eq!(
        (a_row, b_row),
        (4002, 4002),
        "the row_id is the same option for both"
    );
}

#[test]
fn a_quest_taken_while_the_window_is_open_does_not_shift_the_click() {
    use lyracore_shared::constants::{gossip_condition, gossip_option};
    let mut s = quest_store();
    let mut gated = opt(0, "About that favor...", gossip_option::GOSSIP);
    gated.row_id = 4001;
    gated.cond_type = gossip_condition::QUEST_TAKEN;
    gated.cond_value1 = 60;
    let mut inn = opt(0, "Stay here.", gossip_option::INNKEEPER);
    inn.row_id = 4002;
    s.gossip_opts = vec![gated, inn];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    // Rendered while the quest is untaken: the gated line is hidden, "Stay here." is position 0.
    let rendered = gossip_hello(&mut client, &mut c_enc, &mut c_dec, 90);
    assert_eq!(rendered.gossips[0].message, "Stay here.");
    // The player accepts quest 60 elsewhere (another window, a party member's turn-in) — a fresh
    // filter would now put the gated line at 0 and push "Stay here." to 1.
    store.quest_log.lock().unwrap().push((60, false));
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(90),
        gossip_list_id: 0,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE => {}
        other => panic!("expected SMSG_GOSSIP_COMPLETE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert!(
        store.home_bound.load(std::sync::atomic::Ordering::SeqCst),
        "the click must still select the innkeeper line the player was shown"
    );
    assert_eq!(
        store.gossip_selects.lock().unwrap()[0],
        (0, 4002),
        "and the module hears the row the player saw, not the one that moved into that slot"
    );
}

#[test]
fn a_select_with_no_open_menu_just_closes_the_window() {
    use lyracore_shared::constants::gossip_option;
    let mut s = quest_store();
    s.gossip_opts = vec![opt(0, "Stay here.", gossip_option::INNKEEPER)];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    gossip_hello(&mut client, &mut c_enc, &mut c_dec, 90);
    // Same position, a DIFFERENT npc — the open menu is 90's, so this selects nothing.
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(91),
        gossip_list_id: 0,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE => {}
        other => panic!("expected SMSG_GOSSIP_COMPLETE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert!(!store.home_bound.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(
        store.gossip_selects.lock().unwrap()[0].1,
        codec::SYNTHESIZED_ROW_ID,
        "no imported row was selected"
    );
}

#[test]
fn an_imported_menu_missing_its_vendor_row_still_reaches_the_stock() {
    use lyracore_shared::constants::gossip_option;
    let mut s = quest_store();
    s.gossip_opts = vec![opt(0, "What is Children's Week?", gossip_option::GOSSIP)];
    s.vendor_stock = vec![codec::VendorItemView {
        item_entry: 4540,
        display_id: 6353,
        buy_price: 25,
        ..Default::default()
    }];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    let menu = gossip_hello(&mut client, &mut c_enc, &mut c_dec, 90);
    assert_eq!(menu.gossips.len(), 3, "chat + browse goods + Farewell");
    assert_eq!(menu.gossips[0].message, "What is Children's Week?");
    assert_eq!(menu.gossips[1].message, "I'd like to browse your goods.");
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(90),
        gossip_list_id: 1,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (op, body) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(op, codec::SMSG_LIST_INVENTORY_OPCODE);
    assert_eq!(&body[0..8], &90u64.to_le_bytes());
    drop(client);
    server.join().unwrap();
}

#[test]
fn an_imported_menu_missing_its_bind_row_still_offers_the_hearth() {
    use lyracore_shared::constants::gossip_option;
    let mut s = quest_store();
    s.gossip_opts = vec![opt(0, "Tell me about the inn.", gossip_option::GOSSIP)];
    s.innkeeper = true;
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    let menu = gossip_hello(&mut client, &mut c_enc, &mut c_dec, 90);
    assert_eq!(menu.gossips[1].message, "Make this inn your home.");
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(90),
        gossip_list_id: 1,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE => {}
        other => panic!("expected SMSG_GOSSIP_COMPLETE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert!(store.home_bound.load(std::sync::atomic::Ordering::SeqCst));
}

/// A `quest_store()` fixture whose logged-in character (guid 1) reports `level`, so
/// `filtered_gossip_options`' level gate has something to read (`quest_store()` itself leaves
/// `characters` empty, which reads as level 0 — every below-10 test can lean on that default).
fn quest_store_at_level(level: u8) -> InMemoryStore {
    InMemoryStore {
        characters: vec![codec::CharacterView {
            guid: 1,
            level,
            ..Default::default()
        }],
        ..quest_store()
    }
}

#[test]
fn gossip_hello_hides_unlearn_talents_below_level_10() {
    // #516: the imported "I wish to unlearn my talents." row (reclassified by the importer to
    // `UNLEARNTALENTS`, since the raw dump column never carries it) must not render for a character
    // who cannot yet have a talent point.
    use lyracore_shared::constants::gossip_option;
    let mut s = quest_store_at_level(5);
    s.gossip_opts = vec![
        opt(0, "I require warrior training.", gossip_option::TRAINER),
        opt(
            0,
            "I wish to unlearn my talents.",
            gossip_option::UNLEARNTALENTS,
        ),
    ];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_GOSSIP_HELLO {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(m) => {
            assert_eq!(
                m.gossips.len(),
                2,
                "training + trailing Farewell only, no unlearn option: {:?}",
                m.gossips
            );
            assert_eq!(m.gossips[0].message, "I require warrior training.");
            assert_eq!(m.gossips[1].message, "Farewell.");
        }
        other => panic!("expected SMSG_GOSSIP_MESSAGE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn gossip_hello_shows_unlearn_talents_at_level_10_and_select_routes_to_reset_talents() {
    use lyracore_shared::constants::gossip_option;
    let mut s = quest_store_at_level(10);
    s.gossip_opts = vec![
        opt(0, "I require warrior training.", gossip_option::TRAINER),
        opt(
            0,
            "I wish to unlearn my talents.",
            gossip_option::UNLEARNTALENTS,
        ),
    ];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_GOSSIP_HELLO {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(m) => {
            assert_eq!(
                m.gossips.len(),
                3,
                "training + unlearn + trailing Farewell: {:?}",
                m.gossips
            );
            assert_eq!(m.gossips[1].message, "I wish to unlearn my talents.");
        }
        other => panic!("expected SMSG_GOSSIP_MESSAGE, got {other}"),
    }
    // Click it (index 1, same list HELLO just rendered) — must route to reset_talents, not just
    // close the window inert.
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(90),
        gossip_list_id: 1,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE => {}
        other => panic!("expected SMSG_GOSSIP_COMPLETE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    let calls = store.reset_talents_calls.lock().unwrap();
    assert_eq!(
        calls.as_slice(),
        &[(7, 1, 90)],
        "reset_talents must have been called with (account_id, self_guid, trainer_guid)"
    );
}

#[test]
fn gossip_hello_hides_a_quest_gated_option_until_the_quest_is_taken() {
    // 217's second acceptance criterion: an option gated on an unaccepted quest stays hidden.
    use lyracore_shared::constants::{gossip_condition, gossip_option};
    let mut s = quest_store();
    s.gossip_opts = vec![opt(0, "About that favor...", gossip_option::GOSSIP)];
    s.gossip_opts[0].cond_type = gossip_condition::QUEST_TAKEN;
    s.gossip_opts[0].cond_value1 = 60;
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_GOSSIP_HELLO {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        // No imported options survive the filter → falls back to the flag-derived synthesis, which
        // (no vendor stock, no innkeeper flag here) is just the Farewell line.
        ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(m) => {
            assert_eq!(
                m.gossips.len(),
                1,
                "the quest-gated option must be hidden: {:?}",
                m.gossips
            );
            assert_eq!(m.gossips[0].message, "Farewell.");
        }
        other => panic!("expected SMSG_GOSSIP_MESSAGE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn gossip_hello_shows_a_quest_gated_option_once_the_quest_is_taken() {
    use lyracore_shared::constants::{gossip_condition, gossip_option};
    let mut s = quest_store();
    s.gossip_opts = vec![opt(0, "About that favor...", gossip_option::GOSSIP)];
    s.gossip_opts[0].cond_type = gossip_condition::QUEST_TAKEN;
    s.gossip_opts[0].cond_value1 = 60;
    s.quest_log = vec![(60, false)].into(); // taken, not yet turned in
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_GOSSIP_HELLO {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(m) => {
            assert_eq!(m.gossips.len(), 2, "{:?}", m.gossips);
            assert_eq!(m.gossips[0].message, "About that favor...");
        }
        other => panic!("expected SMSG_GOSSIP_MESSAGE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn gossip_hello_and_select_option_stay_position_aligned_under_a_hidden_option() {
    // The sharpest trap (danger-zones-adjacent): 3 imported options where the MIDDLE one is
    // quest-gated and hidden. HELLO sends only 2 lines (positions 0,1); a SELECT of position 1 MUST
    // route to the THIRD raw option (innkeeper), not the hidden middle one — proving
    // `filtered_gossip_options` re-derives the IDENTICAL list rather than indexing the raw rows.
    use lyracore_shared::constants::{gossip_condition, gossip_option};
    let mut s = quest_store();
    s.gossip_opts = vec![
        opt(0, "Chat.", gossip_option::GOSSIP), // raw index 0 -> rendered index 0
        opt(0, "Hidden favor.", gossip_option::GOSSIP), // raw index 1 -> HIDDEN (quest-gated)
        opt(0, "Stay here.", gossip_option::INNKEEPER), // raw index 2 -> rendered index 1
    ];
    s.gossip_opts[1].cond_type = gossip_condition::QUEST_TAKEN;
    s.gossip_opts[1].cond_value1 = 60; // never taken in this store
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_GOSSIP_HELLO {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(m) => {
            assert_eq!(m.gossips.len(), 3, "2 visible + Farewell: {:?}", m.gossips); // hidden option excluded
            assert_eq!(m.gossips[0].message, "Chat.");
            assert_eq!(m.gossips[1].message, "Stay here.");
        }
        other => panic!("expected SMSG_GOSSIP_MESSAGE, got {other}"),
    }
    // Click rendered position 1 ("Stay here.") — must bind home, NOT be swallowed by the hidden
    // middle option that was never actually sent to the client.
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(90),
        gossip_list_id: 1,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE => {}
        other => panic!("expected SMSG_GOSSIP_COMPLETE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert!(
        store.home_bound.load(std::sync::atomic::Ordering::SeqCst),
        "position 1 must resolve to the innkeeper option, not the hidden one"
    );
}

#[test]
fn npc_text_query_ships_the_imported_8_slot_view() {
    let mut view = codec::NpcTextView::default();
    view.slots[0] = (
        "Well met.".to_string(),
        "Well met, traveler.".to_string(),
        0.6,
    );
    view.slots[3] = (
        "Watch yourself.".to_string(),
        "Watch yourself.".to_string(),
        0.4,
    );
    let mut s = quest_store();
    s.npc_text_view = Some(view);
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_NPC_TEXT_QUERY {
        text_id: 77,
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_NPC_TEXT_UPDATE(u) => {
            assert_eq!(u.text_id, 77);
            assert_eq!(u.texts[0].texts[0], "Well met.");
            assert_eq!(u.texts[0].probability, 0.6);
            assert_eq!(u.texts[3].texts[0], "Watch yourself.");
            assert_eq!(u.texts[3].probability, 0.4);
            assert_eq!(u.texts[1].probability, 0.0); // untouched slot stays silent
        }
        other => panic!("expected SMSG_NPC_TEXT_UPDATE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn messagechat_say_and_yell_route_to_chat_types_0_and_1() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Say,
        language: Language::Universal,
        message: "hi".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Yell,
        language: Language::Universal,
        message: "HEY".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client); // no reply on success — the speaker sees their line via the broadcast relay
    server.join().unwrap();
    assert_eq!(
        store.chats.lock().unwrap().as_slice(),
        &[(0, 0, "hi".to_string()), (1, 0, "HEY".to_string())],
        "Say → type 0, Yell → type 1, language threaded"
    );
}

#[test]
fn messagechat_dot_say_diverts_to_gm_command_never_touching_chat() {
    // A Say line starting with '.' diverts to gm_command BEFORE send_chat — never a
    // broadcast, never a game_chat_event insert. No reply on success (the command's own effect is its
    // own feedback).
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Say,
        language: Language::Universal,
        message: ".heal".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {} // nothing was sent for a successful dot-command
        other => panic!("expected the sentinel (no reply on gm_command success), got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.gm_commands.lock().unwrap().as_slice(),
        &[".heal".to_string()],
        "raw text, dot included"
    );
    assert!(
        store.chats.lock().unwrap().is_empty(),
        "a dot-command must NEVER reach send_chat"
    );
}

#[test]
fn messagechat_non_dot_say_is_byte_identical_to_before_223() {
    // The 223 divert must be a no-op for ordinary chat: a Say line NOT starting with '.' still routes
    // to send_chat exactly as before, and never touches gm_command.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Say,
        language: Language::Universal,
        message: "hi".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.chats.lock().unwrap().as_slice(),
        &[(0u8, 0u8, "hi".to_string())]
    );
    assert!(
        store.gm_commands.lock().unwrap().is_empty(),
        "a plain Say line must never reach gm_command"
    );
}

#[test]
fn messagechat_dot_say_error_relays_a_system_chat_line_to_the_sender_only() {
    // A rejected dot-command (bad gm_level, unknown command, bad args) is relayed back
    // to the SENDER as a System SMSG_MESSAGECHAT carrying the module's raw message VERBATIM — no
    // "reducer failed" wrapper prefix, no broadcast, no game_chat_event row.
    let mut s = quest_store();
    s.gm_command_error = Some("permission denied".to_string());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Say,
        language: Language::Universal,
        message: ".god".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_MESSAGECHAT(m) => {
            assert_eq!(m.message, "permission denied");
            assert!(
                matches!(
                    m.chat_type,
                    wow_world_messages::vanilla::SMSG_MESSAGECHAT_ChatType::System { .. }
                ),
                "expected a System chat line, got {:?}",
                m.chat_type
            );
        }
        other => panic!("expected SMSG_MESSAGECHAT System, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert!(store.chats.lock().unwrap().is_empty());
}

#[test]
fn force_run_speed_change_ack_is_swallowed_with_no_reply_and_no_session_teardown() {
    // The client's ack to our `.speed`-triggered SMSG_FORCE_RUN_SPEED_CHANGE must be
    // consumed cleanly (no reply, no desync/disconnect) — proven by a sentinel opcode right after it
    // still getting its normal reply on the SAME session.
    use wow_world_messages::vanilla::{
        MovementInfo, MovementInfo_MovementFlags, Vector3d, CMSG_FORCE_RUN_SPEED_CHANGE_ACK,
    };
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_FORCE_RUN_SPEED_CHANGE_ACK {
        guid: Guid::new(1),
        counter: 1,
        info: MovementInfo {
            flags: MovementInfo_MovementFlags::empty(),
            timestamp: 0,
            position: Vector3d {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            orientation: 0.0,
            fall_time: 0.0,
        },
        new_speed: 21.0,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {} // the ack produced no reply of its own
        other => panic!("expected the sentinel (ack swallowed), got {other}"),
    }
    drop(client);
    server.join().unwrap(); // the session ran to a clean close, not a desync teardown
}

#[test]
fn messagechat_whisper_to_an_unknown_player_replies_player_not_found() {
    let mut s = quest_store();
    s.whisper_error = Some("no player by that name".into());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Whisper {
            target_player: "Ghost".into(),
        },
        language: Language::Universal,
        message: "hello?".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec)
        .expect("a refused whisper must answer SMSG_CHAT_PLAYER_NOT_FOUND — nothing arrived")
    {
        ServerOpcodeMessage::SMSG_CHAT_PLAYER_NOT_FOUND(m) => assert_eq!(m.name, "Ghost"),
        other => panic!("expected SMSG_CHAT_PLAYER_NOT_FOUND, got {other}"),
    }
    drop(client);
    drop(server);
}

#[test]
fn messagechat_guild_is_dropped() {
    // Chat types that need a guild system that doesn't exist yet are dropped — no store call, no reply.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Guild,
        language: Language::Universal,
        message: "g".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {} // nothing was sent for guild
        other => panic!("expected the sentinel (guild dropped), got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert!(
        store.chats.lock().unwrap().is_empty(),
        "guild lines never reach send_chat"
    );
}

#[test]
fn messagechat_party_from_a_grouped_caller_routes_to_party_chat() {
    // A grouped caller's `/p` reaches the module's `party_chat` reducer with the
    // typed text; no reply on success (the caller sees their own line via the SAME per-recipient
    // relay a real member would get — the echo the module pushes, not a gateway-built reply).
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Party,
        language: Language::Universal,
        message: "form up".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {} // nothing was sent for a successful /p
        other => panic!("expected the sentinel (no reply on /p success), got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.party_chats.lock().unwrap().as_slice(),
        &["form up".to_string()]
    );
}

#[test]
fn messagechat_party_from_an_ungrouped_caller_replies_not_in_group() {
    // The module's "not in a group" rejection maps to the SAME
    // SMSG_PARTY_COMMAND_RESULT(NotInGroup) line `group_leave`/`group_uninvite` already use for
    // this exact reducer error (the shared `lyracore_shared::group::err::NOT_IN_GROUP` contract).
    let mut s = quest_store();
    s.party_chat_error = Some(lyracore_shared::group::err::NOT_IN_GROUP.to_string());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Party,
        language: Language::Universal,
        message: "hello?".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_PARTY_COMMAND_RESULT(r) => {
            assert_eq!(
                r.result,
                wow_world_messages::vanilla::PartyResult::NotInGroup
            );
        }
        other => panic!("expected SMSG_PARTY_COMMAND_RESULT(NotInGroup), got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert!(
        store.party_chats.lock().unwrap().is_empty(),
        "a rejected /p never records a message"
    );
}

#[test]
fn messagechat_party_other_rejections_are_silently_dropped() {
    // Not-in-world / empty-message rejections (the send_chat-style failures) get NO reply, matching
    // say/yell — only "not in a group" gets a packet back.
    let mut s = quest_store();
    s.party_chat_error = Some("speaker not in world".to_string());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Party,
        language: Language::Universal,
        message: "x".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {} // nothing was sent for the rejection
        other => panic!("expected the sentinel (silently dropped), got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn stale_epoch_logout_skips_the_logout_reducer() {
    // The world-side half of the session-epoch arbitration: when release_session says a newer
    // login superseded this socket, leave_world must NOT call logout (deleting the entity would
    // vanish the LIVE player).
    let mut s = quest_store();
    s.stale_session = true;
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_LOGOUT_REQUEST {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LOGOUT_RESPONSE(_) => {}
        other => panic!("expected SMSG_LOGOUT_RESPONSE, got {other}"),
    }
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LOGOUT_COMPLETE => {}
        other => panic!("expected SMSG_LOGOUT_COMPLETE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert!(
        !store
            .logout_called
            .load(std::sync::atomic::Ordering::SeqCst),
        "a superseded epoch must NOT delete the newer session's entity"
    );
}

// The cross-database transfer TESTS live in `transfer_tests.rs`, but the fixture types
// below stay here: `InMemoryStore`'s own `Store` impl (the `xdb`/`xstep` glue a few hundred lines up)
// and two world-port-abort regression tests earlier in this file construct `FakeShardDb`/`FakeChar`
// directly, so these are a shared fixture rather than section-local. `transfer_tests` reaches them
// the ordinary way private items in this file reach any child module — no `pub(super)` needed.

/// One character's durable state, reduced to what a transfer has to preserve: where it is, and a
/// PAYLOAD marker standing for the character-owned rows (gear/spells/skills/quest log). If the
/// payload does not arrive, the character arrived NAKED — the failure a manifest-only blob has.
#[derive(Clone, Debug, PartialEq)]
struct FakeChar {
    map_id: u32,
    instance_id: u64,
    payload: String,
}

#[derive(Clone, Debug)]
struct FakeEscrow {
    transfer_id: u64,
    character_guid: u64,
    dest_map_id: u32,
    dest_instance_id: u64,
    blob: Vec<u8>,
}

/// EVERY lock on a `FakeShardDb` goes through here, and it is `try_lock`, not `lock`.
///
/// The fake is only ever touched from the test's own thread, so a failed `try_lock` can only mean
/// one thing: one of its own methods is holding that mutex further up the stack. `std::sync::Mutex`
/// is not re-entrant, so the real `lock()` blocks forever — and `cargo test` has no per-test
/// timeout, so the suite HANGS instead of failing. A review of the cross-database transfer driver
/// hit exactly that: an ordering mutation of the driver made the gateway suite hang rather than
/// turn a named test red, which is a coverage failure wearing a pass's clothes — mutation testing
/// exists to catch exactly this. A hang must never be a pass.
///
/// Deliberate simplification: `try_lock` instead of a watchdog thread per lock — it is one line,
/// it fires instantly, and it names the offending mutex in the panic. The ceiling: it would also
/// fire on genuine cross-thread contention, which these tests do not have (one `FakeShardDb` per
/// test, one thread per test). `transfer_tests::no_hang` is the belt-and-braces net for a hang that
/// is NOT a re-entrant lock (an unbounded retry loop in the driver, say).
fn lk<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.try_lock().expect(
        "re-entrant lock on FakeShardDb: a method is already holding this mutex further up the \
         stack. With `lock()` this would be a DEADLOCK and the suite would HANG instead of failing \
         — see the fn doc on `lk`.",
    )
}

/// Realm-core's authoritative party state, as far as the gateway's routing can see it —
/// the module's `realm_group_op` rules, modelled at the granularity the ROUTING depends on:
/// which database the op lands on, who ends up in which group, and who gets notified.
///
/// Same instrument, and same limits, as [`FakeShardDb`]: the module's own reducer bodies cannot run
/// in a gateway test (no `ReducerContext`), so what executes here is the gateway's production
/// routing (`world::party`) against a faithful stand-in for the authority. The rules themselves are
/// the module's to test.
#[derive(Default)]
struct FakeParty {
    next_group_id: u64,
    /// group_id → (leader, loot_method, loot_threshold, master_looter).
    groups: Vec<(u64, u64, u8, u8, u64)>,
    /// (group_id, character_guid), in join order — the order realm-core's roster read returns.
    members: Vec<(u64, u64)>,
    /// (target_guid, inviter_guid) — at most one pending per target, newest wins.
    invites: Vec<(u64, u64)>,
    /// Every op that reached the AUTHORITY: `(op, actor, target, arg_a, arg_b)`. The assertion that
    /// a party op ran on realm-core rather than on the player's shard.
    ops: Vec<(u8, u64, u64, u8, u8)>,
    /// Every notification the authority pushed: `(recipient_guid, kind)` — the relay's input.
    events: Vec<(u64, u8)>,
}

impl FakeParty {
    fn group_of(&self, guid: u64) -> Option<u64> {
        self.members
            .iter()
            .find(|(_, g)| *g == guid)
            .map(|(gid, _)| *gid)
    }

    fn roster(&self, group_id: u64) -> Option<super::party::GroupRoster> {
        let (gid, leader, method, threshold, master) =
            *self.groups.iter().find(|(g, ..)| *g == group_id)?;
        Some(super::party::GroupRoster {
            group_id: gid,
            leader_guid: leader,
            loot_method: method,
            loot_threshold: threshold,
            master_looter_guid: master,
            members: self
                .members
                .iter()
                .filter(|(g, _)| *g == group_id)
                .map(|(_, guid)| *guid)
                .collect(),
        })
    }

    fn push_list(&mut self, group_id: u64) {
        let recipients: Vec<u64> = self
            .members
            .iter()
            .filter(|(g, _)| *g == group_id)
            .map(|(_, guid)| *guid)
            .collect();
        for r in recipients {
            self.events
                .push((r, lyracore_shared::group::event_kind::LIST));
        }
    }

    /// Drop `guid` from its party, applying the module's own disband rule (a party of one is no
    /// party) — the half `sync_mirrors` has to observe to push the right tombstone.
    fn remove_member(&mut self, guid: u64) {
        use lyracore_shared::group::event_kind as kind;
        let Some(group_id) = self.group_of(guid) else {
            return;
        };
        self.members
            .retain(|(g, m)| !(*g == group_id && *m == guid));
        self.events.push((guid, kind::DESTROYED));
        let remaining: Vec<u64> = self
            .members
            .iter()
            .filter(|(g, _)| *g == group_id)
            .map(|(_, m)| *m)
            .collect();
        if remaining.len() < 2 {
            for r in remaining {
                self.events.push((r, kind::DESTROYED));
            }
            self.members.retain(|(g, _)| *g != group_id);
            self.groups.retain(|(g, ..)| *g != group_id);
        } else {
            if let Some(entry) = self.groups.iter_mut().find(|(g, ..)| *g == group_id) {
                if entry.1 == guid {
                    entry.1 = remaining[0]; // longest-standing member inherits
                }
            }
            self.push_list(group_id);
        }
    }
}

/// One SpacetimeDB database, as far as the transfer protocol can see it.
#[derive(Default)]
struct FakeShardDb {
    characters: std::sync::Mutex<std::collections::HashMap<u64, FakeChar>>,
    /// transfer_id → the source escrow (`game_transfer_out`).
    out_rows: std::sync::Mutex<std::collections::HashMap<u64, FakeEscrow>>,
    /// transfer_id → character guid (`game_transfer_in`): on the DESTINATION the arrival copy's
    /// fence, on the SOURCE the gateway's `confirm_import` attestation.
    in_rows: std::sync::Mutex<std::collections::HashMap<u64, u64>>,
    instances: std::sync::Mutex<std::collections::HashSet<u64>>,
    /// Every instance id this database actually SPAWNED a population for — one entry per
    /// spawn, so "the second party member re-created the dungeon" is visible as a duplicate.
    populated: std::sync::Mutex<Vec<u64>>,
    evicted: std::sync::Mutex<Vec<u64>>,
}

impl FakeShardDb {
    fn with_character(guid: u64, c: FakeChar) -> std::sync::Arc<Self> {
        let db = Self::default();
        lk(&db.characters).insert(guid, c);
        std::sync::Arc::new(db)
    }
    fn empty() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self::default())
    }
    fn has(&self, guid: u64) -> bool {
        lk(&self.characters).contains_key(&guid)
    }
    fn get(&self, guid: u64) -> Option<FakeChar> {
        lk(&self.characters).get(&guid).cloned()
    }
    /// A character is LIVE here iff it is durable AND neither escrow row fences it — the module's
    /// `login_allowed` predicate, which is what `player_login` actually gates on.
    fn live(&self, guid: u64) -> bool {
        self.has(guid)
            && !lk(&self.out_rows)
                .values()
                .any(|e| e.character_guid == guid)
            && !lk(&self.in_rows).values().any(|g| *g == guid)
    }
    fn settled(&self) -> bool {
        lk(&self.out_rows).is_empty() && lk(&self.in_rows).is_empty()
    }
}

/// The blob a fake `begin_transfer` produces. Carries the destination and the payload, exactly as
/// the real `ExportBlob` carries `dest_*` + `character_row` + `payload`: cross-database the blob is
/// the ONLY thing that reaches the far side.
fn fake_blob(guid: u64, dest_map: u32, dest_instance: u64, payload: &str) -> Vec<u8> {
    format!("{guid}|{dest_map}|{dest_instance}|{payload}").into_bytes()
}
fn parse_blob(blob: &[u8]) -> (u64, FakeChar) {
    let s = String::from_utf8(blob.to_vec()).expect("fake blob is utf8");
    let parts: Vec<&str> = s.splitn(4, '|').collect();
    (
        parts[0].parse().expect("guid"),
        FakeChar {
            map_id: parts[1].parse().expect("map"),
            instance_id: parts[2].parse().expect("instance"),
            payload: parts[3].to_string(),
        },
    )
}

// ── Inventory dispatch (CMSG_SWAP_INV_ITEM / CMSG_AUTOSTORE_BAG_ITEM / CMSG_SWAP_ITEM /
// CMSG_SELL_ITEM / CMSG_REPAIR_ITEM / CMSG_TRAINER_LIST) ───────────────────────────────────────────
//
// The 2026-08-10 thermo review found these 13 opcodes with zero offline coverage — first-hour
// gameplay (moving items around the backpack, selling/repairing at a vendor, dying and coming back)
// with no wire-to-store test. Each test below drives `enter_world` + a real CMSG frame and asserts
// either the fake `WorldStore` recorded the RIGHT call+args, or the client got the RIGHT SMSG — the
// same shape as `use_item_without_start_quest_falls_through_to_the_ordinary_use_path` above.

#[test]
fn swap_inv_item_dispatches_move_item_with_the_wire_slots() {
    // CMSG_SWAP_INV_ITEM drives move_item directly with the two ItemSlot wire values decoded to
    // their u8 ordinals — no guid resolution needed (unlike sell/repair, which carry an item guid).
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_SWAP_INV_ITEM {
        source_slot: ItemSlot::MainHand,
        destination_slot: ItemSlot::Inventory1,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client); // move_item (Ok) sends no SMSG on this path
    server.join().unwrap();
    assert_eq!(
        store.moved_items.lock().unwrap().as_slice(),
        &[(ItemSlot::MainHand.as_int(), ItemSlot::Inventory1.as_int())]
    );
}

#[test]
fn swap_inv_item_err_sends_inventory_change_failure() {
    // The equip-slot-transition validation lives in the module; a rejection (e.g. wrong item type
    // for that slot) must reach the client as SMSG_INVENTORY_CHANGE_FAILURE, exactly like the
    // AUTOEQUIP arm's own error test above.
    let mut s = quest_store();
    s.trade_error = Some("cannot equip that there".into());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_SWAP_INV_ITEM {
        source_slot: ItemSlot::Inventory0,
        destination_slot: ItemSlot::MainHand,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(_) => {} // correct feedback packet
        other => panic!("expected SMSG_INVENTORY_CHANGE_FAILURE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn autostore_bag_item_dispatches_unequip_item_for_an_equipped_slot() {
    // Right-clicking an EQUIPPED item (source_bag 255 = main bag, source_slot within the equipment
    // range 0..=EQUIPMENT_SLOT_END) unequips it into the backpack.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_AUTOSTORE_BAG_ITEM {
        source_bag: 255,
        source_slot: 16, // off-hand — inside 0..=18
        destination_bag: 255,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(store.unequipped_slots.lock().unwrap().as_slice(), &[16]);
}

#[test]
fn autostore_bag_item_from_a_backpack_slot_is_unsupported_and_does_not_unequip() {
    // Slot 24 is a BACKPACK slot (>= 23, past EQUIPMENT_SLOT_END=18) — the handler's own guard must
    // refuse it rather than blindly forwarding to unequip_item (there is nothing to unequip there).
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_AUTOSTORE_BAG_ITEM {
        source_bag: 255,
        source_slot: 24,
        destination_bag: 255,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert!(
        store.unequipped_slots.lock().unwrap().is_empty(),
        "a backpack slot must not be routed through unequip_item"
    );
}

#[test]
fn swap_item_within_the_main_bag_dispatches_move_item_via_the_typo_field() {
    // gtker's generated field is spelled `destionation_slot` (a typo baked into the wire crate) — a
    // test that used the correctly-spelled name wouldn't compile, so this pins that the GATEWAY reads
    // the field the wire actually carries.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_SWAP_ITEM {
        source_bag: 255,
        source_slot: 23,
        destination_bag: 255,
        destionation_slot: 30,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(store.moved_items.lock().unwrap().as_slice(), &[(23, 30)]);
}

#[test]
fn swap_item_across_containers_is_unsupported_and_does_not_move() {
    // Only the main inventory (bag 255) is modeled; a swap touching an equipped sub-bag (19..=22)
    // must be refused rather than corrupting an unmodeled container.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_SWAP_ITEM {
        source_bag: 19,
        source_slot: 0,
        destination_bag: 255,
        destionation_slot: 23,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert!(store.moved_items.lock().unwrap().is_empty());
}

#[test]
fn sell_item_resolves_the_instance_guid_to_its_slot_before_dispatch() {
    // CMSG_SELL_ITEM carries the item's INSTANCE guid, not a slot — the gateway must resolve it via
    // player_items() before calling the module's slot-based sell_item.
    let mut s = quest_store();
    s.player_items_fixture = vec![codec::ItemInstanceView {
        guid: 0x4000_0000_0000_0099,
        slot: 30,
        ..Default::default()
    }];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_SELL_ITEM {
        vendor: Guid::new(555),
        item: Guid::new(0x4000_0000_0000_0099),
        amount: 1,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // 248: a successful sell also pushes the refreshed buyback tab (one raw VALUES frame) — drain it
    // tolerantly, same as `buyback_maps_the_wire_slot_enum_to_zero_based_ring_slots` above.
    let _ = ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec);
    drop(client);
    server.join().unwrap();
    assert_eq!(store.sold_items.lock().unwrap().as_slice(), &[(555, 30)]);
}

#[test]
fn sell_item_for_an_unknown_guid_does_not_dispatch() {
    // No fixture item matches this guid (already sold / never ours) — the gateway must log + ignore
    // rather than calling sell_item with a garbage slot.
    let store = std::sync::Arc::new(quest_store()); // player_items_fixture stays empty
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_SELL_ITEM {
        vendor: Guid::new(555),
        item: Guid::new(0x4000_0000_0000_0099),
        amount: 1,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client); // no match → no sell_item call → no buyback-view push, no SMSG at all
    server.join().unwrap();
    assert!(store.sold_items.lock().unwrap().is_empty());
}

#[test]
fn repair_item_resolves_the_instance_guid_to_its_slot_before_dispatch() {
    let mut s = quest_store();
    s.item_slots = vec![(0x4000_0000_0000_0042, 7)];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_REPAIR_ITEM {
        npc: Guid::new(200),
        item: Guid::new(0x4000_0000_0000_0042),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client); // repair_item (Ok) sends no SMSG on this path
    server.join().unwrap();
    assert_eq!(store.repaired_items.lock().unwrap().as_slice(), &[(200, 7)]);
}

#[test]
fn repair_item_guid_zero_is_repair_all_and_dispatches_the_whole_body_slot() {
    // The client's REPAIR-ALL button sends item guid 0, which the gateway routes to the module's
    // whole-body slot (u8::MAX) WITHOUT going through guid→slot resolution — no items are seeded
    // here, so a wrongly-routed per-item lookup would find nothing and skip the call.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_REPAIR_ITEM {
        npc: Guid::new(200),
        item: Guid::new(0),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.repaired_items.lock().unwrap().as_slice(),
        &[(200, u8::MAX)]
    );
}

#[test]
fn repair_all_err_is_relayed_as_a_system_chat_line_not_silently_swallowed() {
    // #514: a rejected repair-all (NPC gate / range / not-enough-money) used to be logged at debug
    // and dropped, so the player saw nothing at all. It must now reach the client as a self-only
    // SMSG_MESSAGECHAT System line carrying the module's rejection text.
    let mut s = quest_store();
    s.trade_error = Some("not enough money to repair".into());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_REPAIR_ITEM {
        npc: Guid::new(200),
        item: Guid::new(0),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_MESSAGECHAT(m) => {
            assert!(
                matches!(
                    m.chat_type,
                    wow_world_messages::vanilla::SMSG_MESSAGECHAT_ChatType::System { .. }
                ),
                "repair failure relays as a System chat line"
            );
            assert_eq!(m.message, "not enough money to repair");
        }
        other => panic!("expected SMSG_MESSAGECHAT, got {other}"),
    }
    assert!(
        store.repaired_items.lock().unwrap().is_empty(),
        "the failed repair never landed a fake success"
    );
    drop(client);
    server.join().unwrap();
}

#[test]
fn trainer_list_replies_smsg_trainer_list_with_the_fixture_spells() {
    let mut s = quest_store();
    s.trainer_spells = vec![codec::TrainerSpellView {
        spell_id: 100,
        cost: 10,
        required_level: 1,
        player_level: 1,
        known: false,
        profession: false,
    }];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_TRAINER_LIST {
        guid: Guid::new(70),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_TRAINER_LIST(list) => {
            assert_eq!(list.guid, Guid::new(70));
            assert_eq!(list.spells.len(), 1, "the fixture's one spell row");
            assert_eq!(list.spells[0].spell, 100);
        }
        other => panic!("expected SMSG_TRAINER_LIST, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn trainer_list_is_silently_dropped_for_a_player_the_trainer_does_not_serve() {
    let mut s = quest_store();
    s.trainer_refuses_class = true;
    s.trainer_spells = vec![codec::TrainerSpellView {
        spell_id: 100,
        cost: 10,
        required_level: 1,
        player_level: 1,
        known: false,
        profession: false,
    }];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_TRAINER_LIST {
        guid: Guid::new(70),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // The follow-up whose reply we DO expect. Gossip always answers, so reading it back proves the
    // trainer request emitted nothing — and it doubles as the "the NPC still talks to you" check:
    // the class gate removes the training service, not the creature.
    CMSG_GOSSIP_HELLO {
        guid: Guid::new(70),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(_) => {}
        ServerOpcodeMessage::SMSG_TRAINER_LIST(_) => {
            panic!("a trainer that does not serve this class must send NO window")
        }
        other => panic!("expected only the gossip reply, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn gossip_hides_the_train_and_unlearn_options_for_a_class_the_trainer_does_not_serve() {
    use lyracore_shared::constants::gossip_option;
    // Level 20 matters: the respec option is independently hidden below level 10, so at the default
    // fixture level this would pass without the class gate doing any work.
    let mut s = quest_store_at_level(20);
    s.gossip_opts = vec![
        opt(0, "Well met, traveler.", gossip_option::GOSSIP),
        opt(1, "I would like to train.", gossip_option::TRAINER),
        opt(
            0,
            "I wish to unlearn my talents.",
            gossip_option::UNLEARNTALENTS,
        ),
        opt(1, "I'd like to browse your goods.", gossip_option::VENDOR),
    ];
    s.trainer_refuses_class = true;
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_GOSSIP_HELLO {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(m) => {
            let lines: Vec<&str> = m.gossips.iter().map(|g| g.message.as_str()).collect();
            assert!(
                !lines.contains(&"I would like to train."),
                "the train option must be hidden: {lines:?}"
            );
            assert!(
                !lines.contains(&"I wish to unlearn my talents."),
                "the respec option must be hidden too: {lines:?}"
            );
            // The NPC is not silenced — it still talks, and still sells.
            assert!(
                lines.contains(&"Well met, traveler."),
                "plain gossip lines survive: {lines:?}"
            );
            assert!(
                lines.contains(&"I'd like to browse your goods."),
                "the vendor line on the same NPC survives: {lines:?}"
            );
        }
        other => panic!("expected SMSG_GOSSIP_MESSAGE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn gossip_keeps_the_train_and_unlearn_options_for_a_class_the_trainer_serves() {
    use lyracore_shared::constants::gossip_option;
    // Same level as its counterpart, so the only difference between the two tests is the gate.
    let mut s = quest_store_at_level(20);
    s.gossip_opts = vec![
        opt(0, "Well met, traveler.", gossip_option::GOSSIP),
        opt(1, "I would like to train.", gossip_option::TRAINER),
        opt(
            0,
            "I wish to unlearn my talents.",
            gossip_option::UNLEARNTALENTS,
        ),
        opt(1, "I'd like to browse your goods.", gossip_option::VENDOR),
    ];
    let store = std::sync::Arc::new(s); // trainer_refuses_class stays false (derive-Default)
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_GOSSIP_HELLO {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(m) => {
            let lines: Vec<&str> = m.gossips.iter().map(|g| g.message.as_str()).collect();
            assert!(
                lines.contains(&"I would like to train."),
                "a served class still gets the train option: {lines:?}"
            );
            assert!(
                lines.contains(&"I wish to unlearn my talents."),
                "and the respec option: {lines:?}"
            );
        }
        other => panic!("expected SMSG_GOSSIP_MESSAGE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

// ── Death/resurrection dispatch (CMSG_REPOP_REQUEST / CMSG_RECLAIM_CORPSE /
// CMSG_RESURRECT_RESPONSE / CMSG_SPIRIT_HEALER_ACTIVATE) ───────────────────────────────────────────

#[test]
fn repop_request_dispatches_repop_for_the_caller() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_REPOP_REQUEST {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drop(client); // repop's revive replicates via the entity VALUES relay, not a direct SMSG here
    server.join().unwrap();
    assert_eq!(store.repopped.lock().unwrap().as_slice(), &[1]);
}

#[test]
fn reclaim_corpse_dispatches_with_the_wire_corpse_guid() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_RECLAIM_CORPSE {
        guid: Guid::new(777),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.reclaimed_corpses.lock().unwrap().as_slice(),
        &[(1, 777)]
    );
}

#[test]
fn resurrect_response_accept_maps_status_byte_to_true() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_RESURRECT_RESPONSE {
        guid: Guid::new(42),
        status: 1,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.resurrect_responses.lock().unwrap().as_slice(),
        &[(1, true)]
    );
}

#[test]
fn resurrect_response_decline_maps_status_byte_to_false() {
    // Proves the `status != 0` mapping actually distinguishes decline from accept, not just that
    // SOME boolean reaches the store.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_RESURRECT_RESPONSE {
        guid: Guid::new(42),
        status: 0,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.resurrect_responses.lock().unwrap().as_slice(),
        &[(1, false)]
    );
}

#[test]
fn spirit_healer_activate_dispatches_and_confirms_the_healer_guid() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_SPIRIT_HEALER_ACTIVATE {
        guid: Guid::new(888),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_SPIRIT_HEALER_CONFIRM(p) => {
            assert_eq!(p.guid, Guid::new(888), "echoes the healer's own guid");
        }
        other => panic!("expected SMSG_SPIRIT_HEALER_CONFIRM, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    // The SMSG above echoes the WIRE guid verbatim, so it alone can't catch a swapped-argument bug —
    // this pins that the STORE call also got (self_guid, healer_guid) in the right order.
    assert_eq!(
        store.spirit_healer_calls.lock().unwrap().as_slice(),
        &[(1, 888)]
    );
}

// ── Targeting/aura dispatch (CMSG_SET_SELECTION / CMSG_CANCEL_AURA / CMSG_CANCEL_CAST) ─────────────

#[test]
fn reducer_transport_loss_ends_an_admitted_session_and_frees_one_queue_seat() {
    let relay_stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let store = std::sync::Arc::new(InMemoryStore {
        login_entity: Some(warrior_entity()),
        set_target_error: Some("transport disconnected".into()),
        // The same dead transport makes leave-world cleanup unreachable. Teardown is best-effort,
        // but the client session and its admission seat must not wait for that reducer.
        logout_error: Some("transport disconnected".into()),
        relay_stopped: Some(relay_stopped.clone()),
        ..tester_store(7)
    });
    let queue = std::sync::Arc::new(LoginQueue::new(1, 0));
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server_queue = queue.clone();
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result =
            run_world_session_with_queue(server_end, server_store.as_ref(), server_queue.as_ref());
        result_tx.send(result).unwrap();
    });

    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    for _ in 0..10 {
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    }
    assert_eq!(
        queue.active(),
        1,
        "the admitted session holds the only seat"
    );

    CMSG_SET_SELECTION {
        target: Guid::new(321),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();

    let err = result_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("transport loss must end the session promptly")
        .expect_err("a disconnected reducer transport must end the world session");
    assert!(format!("{err:#}").contains("transport disconnected"));
    assert!(
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).is_err(),
        "the world socket closes after the fatal reducer result"
    );
    assert!(
        store
            .logout_called
            .load(std::sync::atomic::Ordering::SeqCst),
        "teardown still attempts leave-world cleanup"
    );
    assert!(
        relay_stopped.load(std::sync::atomic::Ordering::SeqCst),
        "session teardown removes local relays"
    );
    assert_eq!(queue.active(), 0, "the ended session released its seat");
    assert_eq!(
        queue.request(),
        Admission::Admitted,
        "one replacement session is admitted"
    );
    assert!(
        matches!(queue.request(), Admission::Queued(_)),
        "only one seat was released"
    );
    drop(client);
}

#[test]
fn set_selection_dispatches_set_target_with_the_wire_guid() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_SET_SELECTION {
        target: Guid::new(321),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(store.selected_targets.lock().unwrap().as_slice(), &[321]);
}

#[test]
fn cancel_aura_dispatches_with_the_wire_spell_id() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_CANCEL_AURA { id: 5555 }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(store.cancelled_auras.lock().unwrap().as_slice(), &[5555]);
}

#[test]
fn cancel_cast_dispatches_for_the_caller() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_CANCEL_CAST { id: 133 }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(store.cancelled_casts.lock().unwrap().as_slice(), &[1]);
}
