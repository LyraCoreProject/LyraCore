use super::*;
use std::os::unix::net::UnixStream;

/// #22 (group slice): the realm-wide party routing tests. A child module so they can reach
/// `InMemoryStore` and its fake realm-core topology without widening anything, kept in their own
/// file because this one is already the largest in the tree.
#[path = "party_tests.rs"]
mod party_tests;

/// #22 (whisper slice): the realm-wide whisper routing tests. A sibling of `party_tests` for the
/// same reason — it reaches `InMemoryStore` (and `party_tests`' live topology) without widening
/// anything.
#[path = "whisper_tests.rs"]
mod whisper_tests;

/// #50: the realm-wide loot-roll routing/relay tests. A sibling of `party_tests`/`whisper_tests` for
/// the same reason — it reaches `InMemoryStore` without widening anything.
#[path = "loot_tests.rs"]
mod loot_tests;

/// #223: the inbound FRAMING boundary — malformed, truncated, oversized and unsupported packets
/// driven as raw bytes over a real cipher. A sibling of the modules above for the same reason (it
/// reaches `InMemoryStore` and `client_handshake`), kept separate because it is the only file here
/// that writes headers no typed builder can produce.
#[path = "framing_tests.rs"]
mod framing_tests;
use wow_world_base::shared::friend_result_vanilla_tbc::FriendResult;
use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;
use wow_world_messages::vanilla::{
    BuyResult,
    BuybackSlot,
    Class,
    ClientMessage,
    Gender,
    GroupLootSetting,
    ItemQuality,
    Language,
    Level,
    Map,
    Object,
    Race,
    RollVote,
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
    CMSG_AUTOEQUIP_ITEM,
    CMSG_BUYBACK_ITEM,
    CMSG_BUY_ITEM,
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
    CMSG_TRAINER_BUY_SPELL,
    // Work-item 194: item-starts-quest + party sharing.
    CMSG_USE_ITEM,
    CMSG_WHO,
    // Work-item 224 (cross-map teleport): the client's world-port-finished ack.
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
    /// WORLDPORT_ACK gate (224): true = entity present -> a spurious ack is ignored;
    /// false (derive-Default) = absent -> a genuine transfer is pending.
    entity_in_world: bool,
    username: String,
    session: Option<WorldSession>,
    characters: Vec<codec::CharacterView>,
    login_entity: Option<codec::EntityView>,
    moves: std::sync::Mutex<Vec<MoveRecord>>,
    /// Vendor stock returned by `vendor_items` (empty by default).
    vendor_stock: Vec<codec::VendorItemView>,
    /// 195: `npc_refuses_interaction` return — false (derive-Default) keeps every fixture NPC open.
    npc_refuses: bool,
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
    /// Recorded `delete_character` calls: (account_id, character_guid). [081]
    deleted: std::sync::Mutex<Vec<(u64, u64)>>,
    /// When set, `delete_character` returns this outcome instead of `Success`.
    delete_outcome: Option<codec::CharDeleteOutcome>,
    /// #60: characters actually produced by a `create_character` call during the test, unioned
    /// into `characters()`'s answer. Without this, a CMSG_CHAR_CREATE round trip is a no-op the
    /// fake immediately forgets, so a test driving CREATE then CMSG_CHAR_ENUM/CMSG_PLAYER_LOGIN
    /// for "the character just created" would actually be exercising a hardcoded/pre-seeded guid
    /// with no real link to the CREATE call — the tautology issue #60's review caught.
    created_characters: std::sync::Mutex<Vec<codec::CharacterView>>,
    /// #60: the Nth guid `create_character` assigns, offset well above every hand-seeded fixture
    /// guid in this file (the highest is 100, in the transfer tests) so it can never collide.
    next_created_guid: std::sync::atomic::AtomicU64,
    /// Reputation standings `player_reputations` returns — `(reputation_index, standing)` pairs folded
    /// into the login SMSG_INITIALIZE_FACTIONS (#13 slice 2, work-item 076).
    reputations: Vec<(i32, i32, bool)>,
    /// Imported action-bar rows `player_actions` returns — `(button, action, action_type)` triples
    /// (work-item 212). Empty by default (the pre-import fallback path).
    player_actions: Vec<(u8, u32, u8)>,
    /// Friend/ignore rows (work-item 130): `(owner_guid, target_guid, is_ignore)`. `add_friend`/
    /// `add_ignore`/`del_friend`/`del_ignore` mutate it; `contact_lists` reads it scoped to the caller.
    contacts: std::sync::Mutex<Vec<(u64, u64, bool)>>,
    group_invites: std::sync::Mutex<Vec<u64>>,
    /// When set, `start_attack` returns this error (drives the ATTACKSWING dead/friendly/desync split). [179]
    start_attack_error: Option<String>,
    /// When set, `start_ranged_attack` returns this error (Auto Shot failure → SMSG_CAST_RESULT). [179]
    start_ranged_attack_error: Option<String>,
    /// When set, `cast_spell` returns this error (cast rejection → SMSG_CAST_RESULT Failure). [179]
    cast_spell_error: Option<String>,
    /// When set, `send_whisper` returns this error (→ SMSG_CHAT_PLAYER_NOT_FOUND). [179]
    whisper_error: Option<String>,
    /// #22 (whisper slice): recorded `send_whisper` calls — `(target_player, message)`, the TYPED
    /// NAME as the pre-#22 path passes it (the module resolves it). The single-database plane's
    /// byte-identity is asserted against this.
    whispers: std::sync::Mutex<Vec<(String, String)>>,
    /// When set, `party_chat` returns this error (work-item 199) — e.g. `group_err::NOT_IN_GROUP`
    /// to drive the "not in a group" → `SMSG_PARTY_COMMAND_RESULT(NotInGroup)` mapping.
    party_chat_error: Option<String>,
    /// Recorded `party_chat` messages (work-item 199) — the dispatch test asserts the RIGHT text
    /// reached the reducer call.
    party_chats: std::sync::Mutex<Vec<String>>,
    /// When set, `gm_command` returns this error (work-item 223) — e.g. `"permission denied"` to
    /// drive the Say-handler's `Err` → self-only `SMSG_MESSAGECHAT` System relay.
    gm_command_error: Option<String>,
    /// Recorded `gm_command` dispatches (work-item 223) — the dot-command divert test asserts the
    /// RIGHT raw text (still carrying its leading `.`) reached the reducer call, and that a NON-dot
    /// Say never reaches this vec at all.
    gm_commands: std::sync::Mutex<Vec<String>>,
    /// Recorded `cast_spell` dispatches: (spell_id, target_guid) — pins target threading. [179]
    casts: std::sync::Mutex<Vec<(u32, u64)>>,
    // Test recorder: the tuple is the recorded CALL's argument list, so it tracks the verb it records.
    #[allow(clippy::type_complexity)]
    /// Ground-targeted casts routed via `cast_spell_at`: (spell_id, target_guid, x, y, z). [118 phase 2]
    ground_casts: std::sync::Mutex<Vec<(u32, u64, f32, f32, f32)>>,
    /// Recorded `start_ranged_attack` dispatches: (target_guid, spell_id) — the Auto Shot intercept. [179]
    ranged_attacks: std::sync::Mutex<Vec<(u64, u32)>>,
    /// What `spell_cast_time` returns: None (default) = unknown spell (the handler treats it as
    /// instant), Some(t) = the game_spell header's cast_time_ms. [179]
    cast_time_ms: Option<u32>,
    queues_next_swing: bool,
    channel_joins: std::sync::Mutex<Vec<String>>,
    channel_messages: std::sync::Mutex<Vec<(String, String)>>,
    /// Enchant/disenchant routing `enchant_route` returns (None = a normal cast). [179]
    enchant_route: Option<super::EnchantRoute>,
    /// Item-guid → bag-slot fixture backing `item_slot_by_guid`. [179]
    item_slots: Vec<(u64, u8)>,
    /// Recorded `enchant_item_on_slot` calls: (slot, enchant_id). [179]
    enchanted: std::sync::Mutex<Vec<(u8, u32)>>,
    /// Recorded `disenchant_item` slots. [179]
    disenchanted: std::sync::Mutex<Vec<u8>>,
    /// The lootable copper `loot_target_money` reports for any target (default 0). [179]
    corpse_money: u32,
    /// Recorded `loot_money` targets — CMSG_LOOT_MONEY must drive the TRACKED guid. [179]
    money_looted: std::sync::Mutex<Vec<u64>>,
    /// Recorded `skin_corpse` targets (the empty-loot-window skinning fallback). [179]
    skinned: std::sync::Mutex<Vec<u64>>,
    /// Recorded `buyback_item` calls: (vendor_guid, slot) — pins the 69→0 slot mapping. [179]
    bought_back: std::sync::Mutex<Vec<(u64, u8)>>,
    /// What `talent_grant_spell` returns (0 = passive talent → no SMSG_LEARNED_SPELL push). [179]
    talent_grant: u32,
    /// What `talent_pane_sync` returns: (teach rank-spell, superseded prev, points remaining).
    talent_pane: (u32, u32, u32),
    /// Spell ids `spell_is_fishing` claims (060).
    fishing_spells: Vec<u32>,
    /// Count of `fish` reducer dispatches (060).
    fish_casts: std::sync::atomic::AtomicU64,
    /// Spell ids `spell_is_open_lock` claims (Pick Lock, 119).
    open_lock_spells: Vec<u32>,
    /// Recorded `pick_lock` reducer dispatches: the target GO guid decoded off the cast (119).
    pick_lock_casts: std::sync::Mutex<Vec<u64>>,
    /// `npc_is_innkeeper` flag for the gossip bind-home routing. [179]
    innkeeper: bool,
    /// Whether `bind_home` ran (the innkeeper gossip select). [179]
    home_bound: std::sync::atomic::AtomicBool,
    /// Recorded `send_chat` lines: (chat_type, language, message). [179]
    chats: std::sync::Mutex<Vec<(u8, u8, String)>>,
    /// When true, `release_session` reports the epoch superseded (stale socket) — the world-side
    /// half of #42: `leave_world` must then SKIP the `logout` reducer. [179]
    stale_session: bool,
    /// #447: the REAL per-account live-socket arbitration (`crate::stdb::AccountSessions`), not a
    /// re-implementation of it — a fake that reimplements the gate only ever tests the fake. The
    /// production `Coordinator` impl runs this exact type behind the exact same predicate; the only
    /// thing stubbed here is the release ACTION (recording instead of closing a websocket).
    account_sessions: crate::stdb::AccountSessions,
    /// Accounts whose cached per-account connection was released, in order (#447). Must stay empty
    /// while ANY socket for the account is still live.
    released_conns: std::sync::Mutex<Vec<u64>>,
    /// Imported gossip menu options `gossip_options` returns for ANY npc_guid (work-item 217) — empty
    /// by default (the pre-217 fallback path).
    gossip_opts: Vec<codec::GossipOptionView>,
    /// The caller's quest log for `quest_status`, as `(quest_id, rewarded)` pairs — a quest id present
    /// here is "taken"; `rewarded` distinguishes active vs. turned-in. Absent = never seen (217).
    quest_log: Vec<(u32, bool)>,
    /// The `npc_text_for_id` view `npc_text_for_id` returns for ANY text_id — `None` by default (the
    /// generic-greeting fallback), settable per-test for the 8-slot pin coverage (217).
    npc_text_view: Option<codec::NpcTextView>,
    /// Per-VIEWER corpse loot fixture for `corpse_loot(corpse_guid, viewer_guid)` (work-item 187 slice
    /// 0): different viewers of the SAME corpse can now see different windows (`quest_only` rows are
    /// per-looter) — keyed by viewer guid, standing in for whatever the real per-viewer read
    /// (`gateway/src/stdb/reads.rs::corpse_loot`) would return for that viewer; its own filtering
    /// decision is unit-tested directly in `reads.rs`, not reproduced here. Empty by default — every
    /// pre-187 test that never sets this keeps seeing an empty window, byte-identical to before.
    corpse_loot_by_viewer: std::collections::HashMap<u64, Vec<codec::LootItemView>>,
    /// Recorded `group_loot_method` calls: (loot_setting, master_guid, loot_threshold) — work-item 187 slice 1.
    group_loot_methods: std::sync::Mutex<Vec<(u8, u64, u8)>>,
    /// Recorded `loot_roll` calls: (corpse_guid, loot_slot, vote) — work-item 187 slices 2-3.
    loot_rolls: std::sync::Mutex<Vec<(u64, u32, u8)>>,
    /// Recorded `loot_master_give` calls: (corpse_guid, loot_slot, target_guid) — work-item 187 slice 4.
    loot_master_gives: std::sync::Mutex<Vec<(u64, u8, u64)>>,
    /// `item_start_quest` fixture (work-item 194: item-starts-quest) — `Some((item_guid, quest_id))`
    /// makes CMSG_USE_ITEM open the quest details screen instead of consuming the item; `None`
    /// (default) is the pre-194 behavior (every item goes through the normal `use_item` consume path).
    item_start_quest_fixture: Option<(u64, u32)>,
    /// Recorded `use_item` slots — work-item 194's non-consumption test proves this stays EMPTY when
    /// `item_start_quest_fixture` intercepts the use.
    used_items: std::sync::Mutex<Vec<u8>>,
    /// Recorded `push_quest` calls: (account_id, quest_id) — work-item 194 (sharing).
    pushed_quests: std::sync::Mutex<Vec<(u64, u32)>>,
    /// When set, `push_quest` returns this error instead of `Ok` — work-item 194.
    push_quest_error: Option<String>,
    /// Recorded `player_login` call count — work-item 224's WORLDPORT_ACK test distinguishes the
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
    /// Recorded `subscribe_player_events` calls: (self_guid, login_map, login_x, login_y) — work-item
    /// 224's WORLDPORT_ACK test asserts this fires AGAIN (a fresh `created` dedup set) at the new
    /// map/position rather than reusing the old subscription.
    subscribed: std::sync::Mutex<Vec<(u64, u32, f32, f32)>>,
    /// The egress DEPTH counter of the live session `subscribe_player_events` was handed — so a test
    /// can read the real queue depth of a real `run_world_session` (the writer thread's decrement has
    /// no other reachable seam: it lives inside the spawned writer loop). Deliberately the depth
    /// `Arc` and NOT the `SessionTx` itself: holding a sender clone here would keep the writer's
    /// `rx.recv()` alive forever and hang every `enter_world` test's `server.join()`.
    session_depth: std::sync::Mutex<Option<std::sync::Arc<std::sync::atomic::AtomicUsize>>>,
    /// Multi-shard routing (#17): the database this handle stands for. `""` (derive-Default) is the
    /// single-shard world every other test runs in, where nothing routes.
    shard: String,
    /// The handle `home_shard()` hands back — the character's home shard. `None` (default) = "you
    /// are already on the right shard", i.e. the single-entry shard map / pre-sharding behavior.
    home: Option<std::sync::Arc<InMemoryStore>>,
    /// Region assignment flip (#23): when set, every `home_shard()` resolution AFTER the first
    /// answers THIS shard instead of `home` — the mock's stand-in for an operator bumping a
    /// region's epoch between two logins. `None` (default): every resolution answers `home`,
    /// byte-identical to before this field existed.
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
    /// #19: the fake DATABASE this handle talks to, for the cross-database transfer tests. `None`
    /// (the default) leaves every transfer trait method at its "this store does not shard" default,
    /// so every other test in this file is untouched.
    xdb: Option<std::sync::Arc<FakeShardDb>>,
    /// #22 (group slice): the handle `realm_store()` hands back — the database that owns party
    /// membership realm-wide. `None` (derive-Default) is the SINGLE-DATABASE gateway, which is what
    /// every other test in this file is, and it is what routes every party op back onto the
    /// player-facing reducers below.
    realm: Option<std::sync::Arc<InMemoryStore>>,
    /// #22: the connected WORLD shards `world_stores()` fans the roster mirror out to (and the
    /// cross-shard name/presence lookups walk). Empty = single database. Behind a `Mutex` only so
    /// the topology can be wired up AFTER every handle exists — production reads the shared
    /// `ShardSet`, which has the same shape and the same "includes this handle" membership.
    peers: std::sync::Mutex<Vec<std::sync::Arc<InMemoryStore>>>,
    /// #22: the AUTHORITATIVE party state, when this handle is the realm-core one. Shared with
    /// nobody — a realm handle owns exactly one of these, and every shard reads its own `mirror`.
    party: std::sync::Arc<std::sync::Mutex<FakeParty>>,
    /// #22: true when this handle is realm-core, so `group_roster` answers from `party` (the
    /// authority) instead of `mirror` (this shard's cache of it).
    is_realm: bool,
    /// #22: what `sync_group_mirror` wrote onto THIS shard, latest per group. The invalidation
    /// story, made observable.
    mirror: std::sync::Mutex<Vec<super::party::GroupRoster>>,
    /// #22: guids with a LIVE entity on this shard — the per-guid `entity_in_world` answer a
    /// realm-wide party frame's online flags are built from. Empty = the single `entity_in_world`
    /// flag above decides, as it did before.
    live_guids: Vec<u64>,
    /// #22: seeded characters that are nevertheless OFFLINE, so the invite gate's "player not
    /// online" arm can be driven. Empty = every seeded character is online, as before.
    offline_guids: Vec<u64>,
    /// #22: when set, `sync_group_mirror` fails with this message — a world shard that cannot be
    /// mirrored (an unreachable database), which must not fail a party op realm-core already took.
    mirror_error: Option<String>,
    /// #22 (whisper slice): what `realm_whisper` was asked to deliver on THIS handle —
    /// `(sender_guid, target_guid, message, sender_is_ignored)`. The realm handle owns the list; a
    /// world shard's staying empty is how a test tells "the whisper went to the authority" from "it
    /// quietly went back to being shard-local".
    realm_whispers: std::sync::Mutex<Vec<(u64, u64, String, bool)>>,
    /// #22 (whisper slice): when set, `realm_whisper` fails with this message — an unreachable
    /// realm-core, which must still leave the player with the same refusal packet they always got.
    realm_whisper_error: Option<String>,
    /// #22 (whisper slice): when set, `contact_lists` fails with this message on THIS shard — the
    /// unreachable-database arm of the realm-wide ignore-list union.
    contact_lists_error: Option<String>,
    /// #51: when set, `realm_group_op(ACCEPT, …)` fails with this message. INJECTED because a real
    /// one cannot be staged synchronously: every accept-time refusal the module has (already grouped,
    /// party full, the inviter no longer leads) needs the party to change BETWEEN the invite and the
    /// accept, and the gateway's bot answer runs in the same call as the invite. The failure is still
    /// reachable in production — a concurrent op on another socket — and what it must not do is leave
    /// the invite dialog hanging.
    party_accept_error: Option<String>,
    /// #19: the transfer step to fail at, simulating a gateway killed before that step's
    /// transaction committed. `None` = nothing fails.
    kill_at: Option<String>,
    /// #19: when set, `settle_home_shard` fails with this message (a transfer that could not be
    /// driven — an unreachable destination shard, a refused import).
    settle_error: Option<String>,
    /// #39: how many `settle_home_shard` calls SUCCEED before `settle_error` starts firing. 0
    /// (derive-Default) = the very first one fails, i.e. the login-time failure #19's test drives.
    /// 1 = the login routes fine and the WORLD-PORT's settle is the one that cannot be driven —
    /// the case that hung a real client on its loading screen forever.
    settle_ok_calls: usize,
    /// How many times `settle_home_shard` has been asked (drives `settle_ok_calls`).
    settle_calls: std::sync::atomic::AtomicUsize,
    /// #19: accounts `bind_shard_session` was called for, per shard.
    bound_sessions: std::sync::Mutex<Vec<u64>>,
    /// #34: the REALM-CORE character→shard index this handle's `publish_shard_index` writes. In
    /// production that write goes to a third database (`realm_core()`); here it is just a map, so a
    /// test can assert the drive published the destination it settled on.
    realm_index: std::sync::Mutex<Vec<(u64, u32, u64)>>,
    /// #34: when set, `publish_shard_index` fails with this message — an unreachable realm-core.
    publish_error: Option<String>,
    /// #39: when set, every `movement_update` fails with this message. The case that matters is
    /// `"mover not in world"` — the module's answer for a packet that arrives after
    /// `teleport_player` despawned the entity, i.e. the tail of every cross-map port.
    movement_error: Option<String>,
    // Test recorder: the tuple is `realm_loot_op`'s argument list verbatim.
    #[allow(clippy::type_complexity)]
    /// #50: recorded `realm_loot_op` calls — `(op, corpse_guid, slot, item_entry, actor_guid, vote,
    /// deadline_micros, recipients)` — every arg the gateway's loot-roll routing/relay passed. The
    /// realm handle owns this; a world shard's staying empty is how a test tells "the vote/promotion
    /// went to the authority" from "it stayed shard-local".
    realm_loot_ops: std::sync::Mutex<Vec<(u8, u64, u8, u32, u64, u8, i64, Vec<u64>)>>,
    /// #50: when set, `realm_loot_op` fails with this message.
    realm_loot_op_error: Option<String>,
    /// #50: this WORLD SHARD's staging rolls `pending_local_rolls` answers — the relay's promotion
    /// INPUT. `Mutex`-wrapped (like `mirror`/`realm_whispers`) so a test can set it AFTER the fixture
    /// is wrapped in an `Arc` — every existing party/whisper topology builder hands back `Arc`s.
    /// Empty (derive-Default) = nothing to promote, byte-identical to before this field existed.
    pending_rolls: std::sync::Mutex<Vec<super::loot::PendingLootRoll>>,
    /// #50: recorded `settle_loot_roll` calls on THIS shard — `(corpse_guid, slot, winner_guid)`.
    settled_rolls: std::sync::Mutex<Vec<(u64, u8, u64)>>,
    /// #50: when set, `settle_loot_roll` fails with this message.
    settle_loot_roll_error: Option<String>,
    /// #50: recorded `clear_promoted_loot_roll` calls on THIS shard — the roll ids the relay told
    /// this shard's staging copy to forget after a successful promotion.
    cleared_rolls: std::sync::Mutex<Vec<u64>>,
    /// #50: this REALM-CORE handle's fixture `ROLL_WON` queue — `(corpse_guid, slot, winner_guid)`
    /// triples, in the order they "arrived". `loot_won_since(after_id)` answers every entry whose
    /// 1-based INDEX exceeds `after_id`, and the new watermark is the queue's length — the same
    /// shape the real `game_group_event.id` high-water mark has, without needing a fake event table.
    /// `Mutex`-wrapped for the same after-`Arc`-construction reason as `pending_rolls`.
    won_events: std::sync::Mutex<Vec<(u64, u8, u64)>>,
    /// #72 slice 1: what `region_shard_for_point` answers on every call — `None` (derive-Default) is
    /// "no seam menu / nothing routes", byte-identical to before this field existed.
    region_shard_answer: Option<String>,
    /// #72 slice 1: how many times `region_shard_for_point` was asked — the wiring test's proof that
    /// the movement path calls it, and ONLY on a cell change (not every heartbeat).
    region_shard_calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    /// #72 slice 2: `(db name, handle)` pairs `shard_by_name` resolves from — the mock's stand-in
    /// for `Coordinator::shard_handle`'s connected-shard-set lookup. Empty (derive-Default) means
    /// every name is "not a connected shard", matching a single-database gateway.
    named_shards: std::sync::Mutex<Vec<(String, std::sync::Arc<InMemoryStore>)>>,
}

impl InMemoryStore {
    /// Record one player-scoped call against THIS handle's shard (#17).
    fn rec(&self, what: &str) {
        self.calls
            .lock()
            .unwrap()
            .push((self.shard.clone(), what.to_string()));
    }

    /// Record a transfer step and honour an injected kill (#19). `Err` means "the gateway died
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

    /// #72 slice 1: records the call, then answers the fixed `region_shard_answer` — the mock has
    /// no seam menu/assignment model of its own, so unlike production this never actually looks at
    /// `map_id`/`x`/`y`; the test using this asserts CALL COUNT (the cell-change gate), not content.
    fn region_shard_for_point(
        &self,
        _character_guid: u64,
        _home_db: &str,
        _map_id: u32,
        _x: f32,
        _y: f32,
    ) -> Option<String> {
        self.region_shard_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.region_shard_answer.clone()
    }

    /// #72 slice 2: look up `db` in `named_shards` — the mock's `Coordinator::shard_handle`.
    fn shard_by_name(&self, db: &str) -> Option<std::sync::Arc<dyn WorldStore>> {
        self.named_shards
            .lock()
            .unwrap()
            .iter()
            .find(|(name, _)| name == db)
            .map(|(_, h)| h.clone() as std::sync::Arc<dyn WorldStore>)
    }

    /// #72 slice 2: `self` is a concrete `InMemoryStore` here, so — like the production
    /// `Coordinator` impl — this can hand both ends straight to the real driver.
    fn run_warm_handoff(&self, dst: &dyn WorldStore, plan: &TransferPlan) -> Result<()> {
        super::transfer::run_transfer(self, dst, plan)
    }

    // --- #19: the escrow protocol, with the MODULE's guards reproduced. A permissive mock would
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

    /// #34: the realm-core index publish. Recorded in the shared call log so its POSITION in the
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
        // transfer method routes its "gateway killed here" injection through it, and this one did
        // not when it was added (#34), so `kill_at = "publish_shard_index"` was silently inert and
        // the crash matrix reported a PASS for a boundary it never killed at.
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
        // else success — and a success actually RECORDS the character (#60), assigning it a real
        // guid `characters()` then unions in. Before #60's review this call was a pure no-op the
        // fake immediately forgot, which let a test claim to drive "the character CREATE just
        // produced" while actually logging into an unrelated hardcoded/pre-seeded guid.
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
    fn movement_update(&self, _account_id: u64, opcode: u32, info: &MovementInfo) -> Result<()> {
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
        Ok(PlayerSubscriptions::empty())
    }
    fn logout(&self, _account_id: u64) -> Result<()> {
        self.rec("logout");
        self.logout_called
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
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
    fn use_gameobject(&self, _account_id: u64, _go_guid: u64) -> Result<()> {
        Ok(())
    }
    fn client_command(&self, _account_id: u64, _cmd: String, _payload: String) -> Result<()> {
        Ok(())
    }

    fn enter_areatrigger(&self, _account_id: u64, _trigger_id: u32) -> Result<()> {
        Ok(())
    }
    fn player_items(&self, _owner_guid: u64) -> Result<Vec<codec::ItemInstanceView>> {
        Ok(Vec::new())
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
    fn buy_item(
        &self,
        _account_id: u64,
        _vendor_guid: u64,
        _item_entry: u32,
        _count: u32,
    ) -> Result<()> {
        match &self.trade_error {
            Some(e) => Err(anyhow!("{e}")),
            None => Ok(()),
        }
    }
    fn sell_item(&self, _account_id: u64, _vendor_guid: u64, _slot: u8) -> Result<()> {
        match &self.trade_error {
            Some(e) => Err(anyhow!("{e}")),
            None => Ok(()),
        }
    }
    fn buyback_item(&self, _account_id: u64, vendor_guid: u64, slot: u8) -> Result<()> {
        if let Some(e) = &self.trade_error {
            return Err(anyhow!("{e}"));
        }
        self.bought_back.lock().unwrap().push((vendor_guid, slot));
        Ok(())
    }
    fn repair_item(&self, _account_id: u64, _npc_guid: u64, _slot: u8) -> Result<()> {
        match &self.trade_error {
            Some(e) => Err(anyhow!("{e}")),
            None => Ok(()),
        }
    }
    fn trainer_list(
        &self,
        _player_guid: u64,
        _trainer_guid: u64,
    ) -> Result<Vec<codec::TrainerSpellView>> {
        Ok(Vec::new())
    }
    fn buy_trainer_spell(
        &self,
        _account_id: u64,
        _trainer_guid: u64,
        _spell_id: u32,
    ) -> Result<()> {
        match &self.trade_error {
            Some(e) => Err(anyhow!("{e}")),
            None => Ok(()),
        }
    }
    fn skin_corpse(&self, _account_id: u64, corpse_guid: u64) -> Result<()> {
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
    fn disenchant_item(&self, _account_id: u64, slot: u8) -> Result<()> {
        self.disenchanted.lock().unwrap().push(slot);
        Ok(())
    }
    fn enchant_item_on_slot(&self, _account_id: u64, slot: u8, enchant_id: u32) -> Result<()> {
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
    fn fish(&self, _account_id: u64) -> Result<()> {
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
    fn pick_lock(&self, _account_id: u64, go_guid: u64) -> Result<()> {
        self.pick_lock_casts.lock().unwrap().push(go_guid);
        match &self.trade_error {
            Some(e) => Err(anyhow!("{e}")),
            None => Ok(()),
        }
    }
    fn set_faction_at_war(
        &self,
        _account_id: u64,
        _reputation_index: u32,
        _at_war: bool,
    ) -> Result<()> {
        Ok(())
    }
    fn set_action_button(
        &self,
        _account_id: u64,
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
    fn learn_talent(&self, _account_id: u64, _talent_id: u32) -> Result<()> {
        match &self.trade_error {
            Some(e) => Err(anyhow!("{e}")),
            None => Ok(()),
        }
    }
    fn equip_item(&self, _account_id: u64, _from_slot: u8) -> Result<()> {
        match &self.trade_error {
            Some(e) => Err(anyhow!("{e}")),
            None => Ok(()),
        }
    }
    fn unequip_item(&self, _account_id: u64, _from_slot: u8) -> Result<()> {
        match &self.trade_error {
            Some(e) => Err(anyhow!("{e}")),
            None => Ok(()),
        }
    }
    fn use_item(&self, _account_id: u64, slot: u8) -> Result<()> {
        self.used_items.lock().unwrap().push(slot);
        match &self.trade_error {
            Some(e) => Err(anyhow!("{e}")),
            None => Ok(()),
        }
    }
    fn item_start_quest(&self, _owner_guid: u64, _slot: u8) -> Option<(u64, u32)> {
        self.item_start_quest_fixture
    }
    fn push_quest(&self, account_id: u64, quest_id: u32) -> Result<()> {
        if let Some(e) = &self.push_quest_error {
            return Err(anyhow!("{e}"));
        }
        self.pushed_quests
            .lock()
            .unwrap()
            .push((account_id, quest_id));
        Ok(())
    }
    fn bind_home(&self, _account_id: u64) -> Result<()> {
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
        match self.quest_log.iter().find(|(id, _)| *id == quest_id) {
            Some((_, rewarded)) => (true, *rewarded),
            None => (false, false),
        }
    }
    fn move_item(&self, _account_id: u64, _from_slot: u8, _to_slot: u8) -> Result<()> {
        match &self.trade_error {
            Some(e) => Err(anyhow!("{e}")),
            None => Ok(()),
        }
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
    fn accept_quest(&self, account_id: u64, giver_guid: u64, quest_id: u32) -> Result<()> {
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
        // #22: `live_guids` is the per-guid answer the realm-wide party frame needs ("is this member
        // live on THIS shard"). Empty by default, so the single flag above is still the answer every
        // pre-#22 test set.
        self.entity_in_world || self.live_guids.contains(&guid)
    }
    fn abandon_quest(&self, account_id: u64, quest_id: u32) -> Result<()> {
        if let Some(e) = &self.trade_error {
            return Err(anyhow!("{e}"));
        }
        self.abandoned.lock().unwrap().push((account_id, quest_id));
        Ok(())
    }
    fn set_target(&self, _account_id: u64, _target_guid: u64) -> Result<()> {
        self.rec("set_target");
        Ok(())
    }
    fn inspect(&self, _account_id: u64, target_guid: u64) -> Result<()> {
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
    fn start_attack(&self, _account_id: u64, _target_guid: u64) -> Result<()> {
        self.rec("start_attack");
        match &self.start_attack_error {
            Some(e) => Err(anyhow!("{e}")),
            None => Ok(()),
        }
    }
    fn pet_command(&self, _account_id: u64, _data: u32, _target_guid: u64) -> Result<()> {
        Ok(())
    }
    fn start_ranged_attack(&self, _account_id: u64, target_guid: u64, spell_id: u32) -> Result<()> {
        if let Some(e) = &self.start_ranged_attack_error {
            return Err(anyhow!("{e}"));
        }
        self.ranged_attacks
            .lock()
            .unwrap()
            .push((target_guid, spell_id));
        Ok(())
    }
    fn stop_attack(&self, _account_id: u64) -> Result<()> {
        Ok(())
    }
    fn cast_spell(&self, _account_id: u64, spell_id: u32, target_guid: u64) -> Result<()> {
        if let Some(e) = &self.cast_spell_error {
            return Err(anyhow!("{e}"));
        }
        self.casts.lock().unwrap().push((spell_id, target_guid));
        Ok(())
    }
    fn cast_spell_at(
        &self,
        _account_id: u64,
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
    fn cancel_aura(&self, _account_id: u64, _spell_id: u32) -> Result<()> {
        Ok(())
    }
    fn cancel_cast(&self, _account_id: u64) -> Result<()> {
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
    fn join_channel(&self, _account_id: u64, channel: String) -> Result<()> {
        self.channel_joins.lock().unwrap().push(channel);
        Ok(())
    }
    fn leave_channel(&self, _account_id: u64, _channel: String) -> Result<()> {
        Ok(())
    }
    fn send_channel_message(
        &self,
        _account_id: u64,
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
        chat_type: u8,
        language: u8,
        message: String,
    ) -> Result<()> {
        // #22: recorded per SHARD like every other player-scoped call, so the partition rule (say/
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
        _text_emote: u32,
        _emote_anim: u32,
        _target_guid: u64,
    ) -> Result<()> {
        self.rec("send_emote");
        Ok(())
    }
    fn send_roll(&self, _account_id: u64, _min_roll: u32, _max_roll: u32) -> Result<()> {
        Ok(())
    }
    fn send_whisper(&self, _account_id: u64, target_player: String, message: String) -> Result<()> {
        // #22 (whisper slice): recorded per SHARD, so a test can tell the pre-#22 path (the
        // player-facing reducer on the player's own database, with the TYPED NAME still unresolved)
        // from the realm-core one (`realm_whispers`, by guid).
        self.rec("send_whisper");
        self.whispers.lock().unwrap().push((target_player, message));
        match &self.whisper_error {
            Some(e) => Err(anyhow!("{e}")),
            None => Ok(()),
        }
    }
    fn party_chat(&self, _account_id: u64, message: String) -> Result<()> {
        match &self.party_chat_error {
            Some(e) => Err(anyhow!("{e}")),
            None => {
                self.party_chats.lock().unwrap().push(message);
                Ok(())
            }
        }
    }
    fn gm_command(&self, _account_id: u64, text: String) -> Result<()> {
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
    fn loot_money(&self, _account_id: u64, target_guid: u64) -> Result<()> {
        self.money_looted.lock().unwrap().push(target_guid);
        Ok(())
    }
    fn take_loot(&self, _account_id: u64, _corpse_guid: u64, _loot_slot: u8) -> Result<()> {
        Ok(())
    }
    fn repop(&self, _account_id: u64) -> Result<()> {
        Ok(())
    }
    fn claim_session(&self, _account_id: u64) -> u64 {
        1
    }
    fn release_session(&self, _account_id: u64, _epoch: u64) -> bool {
        // Default (false) = this session still owns the entity; `stale_session` simulates a newer
        // login having superseded it (the #42 arbitration), so teardown must skip `logout`.
        !self.stale_session
    }
    fn open_account_session(&self, account_id: u64) {
        self.account_sessions.attach(account_id);
    }
    fn close_account_session(&self, account_id: u64) {
        // Byte-for-byte the production predicate (`Coordinator::detach_account_session`): release
        // ONLY when this was the account's last live socket.
        if self.account_sessions.detach(account_id) {
            self.released_conns.lock().unwrap().push(account_id);
        }
    }
    fn reclaim_corpse(&self, _account_id: u64, _corpse_guid: u64) -> Result<()> {
        Ok(())
    }
    fn resurrect_response(&self, _account_id: u64, _accept: bool) -> Result<()> {
        Ok(())
    }
    fn spirit_healer_res(&self, _account_id: u64, _healer_guid: u64) -> Result<()> {
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
            // #22: `offline_guids` drives the invite gate's "player not online" arm. Empty by
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
    // Group (066): a minimal in-memory party — enough for the dispatch tests to drive
    // invite-result mapping and the GROUP_LIST build without a live module.
    //
    // #22: each of these records the SHARD it ran on (`rec`), so a test can tell the
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

    // --- #22 (group slice): the realm-core plane ---

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

    // --- #50: realm-wide loot rolls ---

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
        _npc_guid: u64,
        _option_id: u32,
        _option_row_id: u32,
    ) -> Result<()> {
        Ok(())
    }
    fn add_friend(&self, _account_id: u64, target_guid: u64) -> Result<()> {
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
    fn del_friend(&self, _account_id: u64, target_guid: u64) -> Result<()> {
        let owner = self.login_entity.as_ref().map(|e| e.guid).unwrap_or(0);
        let mut contacts = self.contacts.lock().unwrap();
        let before = contacts.len();
        contacts.retain(|&(o, t, ig)| !(o == owner && t == target_guid && !ig));
        if contacts.len() == before {
            return Err(anyhow!("not on that list"));
        }
        Ok(())
    }
    fn add_ignore(&self, _account_id: u64, target_guid: u64) -> Result<()> {
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
    fn del_ignore(&self, _account_id: u64, target_guid: u64) -> Result<()> {
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

fn ns(s: &str) -> NormalizedString {
    NormalizedString::new(s).unwrap()
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
    let server_seed = match ServerOpcodeMessage::read_unencrypted(&mut *client).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_CHALLENGE(c) => c.server_seed,
        other => panic!("expected SMSG_AUTH_CHALLENGE, got {other}"),
    };
    let client_seed = ProofSeed::new();
    let client_seed_value = client_seed.seed();
    let (client_proof, crypto) =
        client_seed.into_client_header_crypto(&ns(username), key, server_seed);
    let (enc, mut dec) = crypto.split();

    auth_session(username, client_seed_value, client_proof)
        .write_unencrypted_client(&mut *client)
        .unwrap();

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

#[test]
fn handshake_succeeds_and_traffic_is_encrypted_both_ways() {
    let store = std::sync::Arc::new(InMemoryStore {
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 42,
            session_key: K,
        }),
        ..Default::default()
    });

    let (mut client, server_end) = UnixStream::pair().unwrap();
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

    // --- client: read the plaintext server challenge, learn the server seed ---
    let server_seed = match ServerOpcodeMessage::read_unencrypted(&mut client).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_CHALLENGE(c) => c.server_seed,
        other => panic!("expected SMSG_AUTH_CHALLENGE, got {other}"),
    };

    // --- client: build the proof + cipher with wow_srp's client side, send AUTH_SESSION ---
    let client_seed = ProofSeed::new();
    let client_seed_value = client_seed.seed();
    let (client_proof, client_crypto) =
        client_seed.into_client_header_crypto(&ns("TESTER"), K, server_seed);
    let (mut c_enc, mut c_dec) = client_crypto.split();

    auth_session("TESTER", client_seed_value, client_proof)
        .write_unencrypted_client(&mut client)
        .unwrap();

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

/// #180 wiring: `world_handshake_with_queue` actually CONSULTS the [`LoginQueue`] gate rather than
/// always admitting outright. A cap of 1 with the one seat already taken must send `AuthWaitQueue {
/// queue_position: 1 }` as the FIRST response — not `AuthOk` — and only send `AuthOk` after the seat
/// frees. This is the assertion that would FAIL if the gate check were ever deleted from
/// `world_handshake_with_queue` (a behavioral mutation on the production path, not merely a
/// structural "does this function exist" check).
#[test]
fn queued_handshake_sends_wait_queue_then_admits_once_a_seat_frees() {
    let store = std::sync::Arc::new(InMemoryStore {
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 42,
            session_key: K,
        }),
        ..Default::default()
    });
    let queue = std::sync::Arc::new(LoginQueue::new(1, 0));
    // Occupy the only seat directly — exactly what an already-connected world session holds.
    assert_eq!(queue.request(), Admission::Admitted);

    let (mut client, server_end) = UnixStream::pair().unwrap();
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

    // --- client: plaintext challenge, then AUTH_SESSION with a valid proof (same shape as the
    // unqueued handshake test above) ---
    let server_seed = match ServerOpcodeMessage::read_unencrypted(&mut client).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_CHALLENGE(c) => c.server_seed,
        other => panic!("expected SMSG_AUTH_CHALLENGE, got {other}"),
    };
    let client_seed = ProofSeed::new();
    let client_seed_value = client_seed.seed();
    let (client_proof, client_crypto) =
        client_seed.into_client_header_crypto(&ns("TESTER"), K, server_seed);
    let (_c_enc, mut c_dec) = client_crypto.split();

    auth_session("TESTER", client_seed_value, client_proof)
        .write_unencrypted_client(&mut client)
        .unwrap();

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

/// #180: a socket that disconnects WHILE QUEUED must leave the line without ever taking a seat — the
/// gate's own unit tests cover `LoginQueue::cancel` directly; this proves `world_handshake_with_queue`
/// actually calls it on a clean hangup (rather than, say, leaking a phantom waiter forever).
#[test]
fn disconnecting_while_queued_leaves_the_line_without_taking_a_seat() {
    let store = std::sync::Arc::new(InMemoryStore {
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        ..Default::default()
    });
    let queue = std::sync::Arc::new(LoginQueue::new(1, 0));
    assert_eq!(queue.request(), Admission::Admitted); // occupy the only seat

    let (mut client, server_end) = UnixStream::pair().unwrap();
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

    let server_seed = match ServerOpcodeMessage::read_unencrypted(&mut client).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_CHALLENGE(c) => c.server_seed,
        other => panic!("expected SMSG_AUTH_CHALLENGE, got {other}"),
    };
    let client_seed = ProofSeed::new();
    let client_seed_value = client_seed.seed();
    let (client_proof, client_crypto) =
        client_seed.into_client_header_crypto(&ns("TESTER"), K, server_seed);
    let (_c_enc, mut c_dec) = client_crypto.split();

    auth_session("TESTER", client_seed_value, client_proof)
        .write_unencrypted_client(&mut client)
        .unwrap();

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
    // #20 AC#2 (the stateless-gateway invariant, now realm-scoped). The session key K lives in
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
        let (mut client, server_end) = UnixStream::pair().unwrap();
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
    // The other half of AC#2: "resume from realm state" must not degrade into "resume from
    // anything". A store that cannot answer (realm-core unreachable → `lookup_session` yields no
    // session) rejects the handshake plaintext instead of establishing a session on an unverified
    // key. `CoordinatorStore` reaches this state by way of `Coordinator::realm_core()`'s Err.
    let store = InMemoryStore {
        username: "TESTER".into(),
        session: None,
        ..Default::default()
    };
    let (mut client, server_end) = UnixStream::pair().unwrap();
    let server = std::thread::spawn(move || {
        let mut s = server_end;
        assert!(
            world_handshake(&mut s, &store).unwrap().is_none(),
            "no session material ⇒ no session, never a best-effort one"
        );
    });

    let server_seed = match ServerOpcodeMessage::read_unencrypted(&mut client).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_CHALLENGE(c) => c.server_seed,
        other => panic!("expected SMSG_AUTH_CHALLENGE, got {other}"),
    };
    let client_seed = ProofSeed::new();
    let client_seed_value = client_seed.seed();
    let (client_proof, _crypto) =
        client_seed.into_client_header_crypto(&ns("TESTER"), K, server_seed);
    auth_session("TESTER", client_seed_value, client_proof)
        .write_unencrypted_client(&mut client)
        .unwrap();
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

    let (mut client, server_end) = UnixStream::pair().unwrap();
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

    let (mut client, server_end) = UnixStream::pair().unwrap();
    let server = std::thread::spawn(move || {
        let mut s = server_end;
        assert!(world_handshake(&mut s, &store).unwrap().is_none());
    });

    let server_seed = match ServerOpcodeMessage::read_unencrypted(&mut client).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_CHALLENGE(c) => c.server_seed,
        other => panic!("expected SMSG_AUTH_CHALLENGE, got {other}"),
    };

    let client_seed = ProofSeed::new();
    let client_seed_value = client_seed.seed();
    let (client_proof, _crypto) =
        client_seed.into_client_header_crypto(&ns("NOBODY"), K, server_seed);

    auth_session("NOBODY", client_seed_value, client_proof)
        .write_unencrypted_client(&mut client)
        .unwrap();

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
    // The pre-seeded Human Warrior "Tester" must appear on the character-select screen (AC#3).
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
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        characters: vec![tester],
        ..Default::default()
    });

    let (mut client, server_end) = UnixStream::pair().unwrap();
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
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        characters: vec![tester],
        ..Default::default()
    });
    let (mut client, server_end) = UnixStream::pair().unwrap();
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

/// CMSG_CHAR_DELETE for an owned guid replies SMSG_CHAR_DELETE(success) and dispatches the
/// (account_id, character_guid) to the store; a store-reported failure still replies (never
/// session-fatal), same treatment as CMSG_CHAR_CREATE. [081]
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
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        characters: vec![tester],
        ..Default::default()
    });
    let (mut client, server_end) = UnixStream::pair().unwrap();
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

/// A store-reported delete failure (e.g. NOT_OWNER/NO_SUCH_CHAR mapped module-side) still
/// replies SMSG_CHAR_DELETE(failed) — the connection is NOT torn down. [081]
#[test]
fn char_delete_failure_replies_failed_and_keeps_session_alive() {
    let store = std::sync::Arc::new(InMemoryStore {
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        delete_outcome: Some(codec::CharDeleteOutcome::Failed),
        ..Default::default()
    });
    let (mut client, server_end) = UnixStream::pair().unwrap();
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
    // self CREATE_OBJECT2 at the correct position/guid (AC#4 wire behavior).
    let store = std::sync::Arc::new(InMemoryStore {
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        login_entity: Some(warrior_entity()),
        ..Default::default()
    });

    let (mut client, server_end) = UnixStream::pair().unwrap();
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
    // Work-item 224: MSG_MOVE_WORLDPORT_ACK must re-run the SAME enter_world path as
    // CMSG_PLAYER_LOGIN — rebuilding the entity (now on the NEW map the module's teleport_player
    // durably wrote to the character row) and re-subscribing with a FRESH `created` dedup set (the
    // "AOI initial-apply" precedent, work-item 145) rather than reusing the old map's subscription —
    // a stale created-set is exactly what would leave entities invisible on cross-map arrival.
    let mut ported = warrior_entity();
    ported.map_id = 1; // Kalimdor — simulates teleport_player's durable cross-map write
    ported.x = 100.0;
    ported.y = 200.0;
    let store = std::sync::Arc::new(InMemoryStore {
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        login_entity: Some(warrior_entity()),
        worldport_entity: Some(ported),
        ..Default::default()
    });

    let (mut client, server_end) = UnixStream::pair().unwrap();
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

    // enter_world reruns the FULL login-style sequence verbatim for the re-entry.
    let mut verify_world_map = None;
    let mut create_guid = None;
    for _ in 0..10 {
        match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
            ServerOpcodeMessage::SMSG_LOGIN_VERIFY_WORLD(m) => {
                verify_world_map = Some(m.map);
                assert!(
                    (m.position.x - 100.0).abs() < 0.01,
                    "the re-entry sequence must use the NEW position"
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
        verify_world_map,
        Some(Map::Kalimdor),
        "the re-entry sequence must reflect the NEW map"
    );
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

    drop(client);
    server.join().unwrap();
}

#[test]
fn login_initialize_factions_carries_persisted_standing_at_its_reputation_index() {
    // Work-item 076: a persisted `game_player_reputation` row must land in the login
    // SMSG_INITIALIZE_FACTIONS at its STORED reputation_index slot (0..63), never faction_id — the
    // guardrail that also gates the live SET_FACTION_STANDING relay (McBride ERROR #132).
    let store = std::sync::Arc::new(InMemoryStore {
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        login_entity: Some(warrior_entity()),
        // Stormwind's rep-index is 19 (Faction.dbc ReputationListID), NOT its faction id (72) —
        // exercising the exact index/id distinction the guardrail protects.
        reputations: vec![(19, 3175, false)],
        ..Default::default()
    });

    let (mut client, server_end) = UnixStream::pair().unwrap();
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
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        ..Default::default()
    });

    let (mut client, server_end) = UnixStream::pair().unwrap();
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

/// #72 slice 1 (seam-crossing DETECTION, no handoff): the movement path must actually CALL
/// `region_shard_for_point` — proving the WIRING, not just the pure hysteresis state machine
/// (`world::seam`'s own unit tests already cover that in isolation) — and it must do so ONLY when
/// the player's cell changed, never on a same-cell heartbeat (the cadence rule: "not every
/// heartbeat"). Each heartbeat below carries a distinct `orientation` so work-item 231's coalescer
/// treats every one as a STATE CHANGE and forwards it immediately (231 rule 1) — the point of this
/// test is the seam gate, not the unrelated coalescing window.
#[test]
fn movement_drives_the_seam_check_only_on_a_cell_change() {
    use wow_world_messages::vanilla::{
        MSG_MOVE_HEARTBEAT_Client, MovementInfo, MovementInfo_MovementFlags, Vector3d,
    };

    let region_shard_calls: std::sync::Arc<std::sync::atomic::AtomicUsize> = Default::default();
    let store = std::sync::Arc::new(InMemoryStore {
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        login_entity: Some(warrior_entity()), // map 0, x=-8949.95, y=-132.493
        region_shard_calls: region_shard_calls.clone(),
        ..Default::default()
    });

    let (mut client, server_end) = UnixStream::pair().unwrap();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.as_ref()).unwrap();
    });

    let (mut c_enc, _c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    let beat = |o: f32, x: f32, y: f32, t: u32| MSG_MOVE_HEARTBEAT_Client {
        info: MovementInfo {
            flags: MovementInfo_MovementFlags::empty(),
            timestamp: t,
            position: Vector3d { x, y, z: 83.5312 },
            orientation: o,
            fall_time: 0.0,
        },
    };
    // 1st packet ever this session: `SeamTracker::last_cell` starts `None`, so this always counts
    // as "changed" — one call, at the login cell.
    beat(1.0, -8949.95, -132.493, 100)
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    // A few yards, same 50yd cell: must NOT call `region_shard_for_point` again.
    beat(1.1, -8945.0, -130.0, 200)
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    // 600yd away — well over a cell boundary: a second call.
    beat(1.2, -8949.95, -132.493 - 600.0, 300)
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    drop(client);
    server.join().unwrap();

    assert_eq!(
        region_shard_calls.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "the seam check must run on cell #1 (login) and cell #2 (600yd away), and NOT on the \
         same-cell heartbeat in between — 3 calls would mean it runs every heartbeat (the cadence \
         rule broken), 1 or 0 would mean it isn't wired into the movement path at all"
    );
}

/// #72 slice 2: a `WorldConn` past the handshake, forced straight into `InWorld` with the given
/// combat state — for testing `handle_seam_crossing`'s guards directly, without a full login round
/// trip (the module/AOI machinery a real login exercises is irrelevant to what these guards read).
/// `store` must carry a matching `username`/`session` (the handshake needs `lookup_session`).
fn make_inworld_conn(
    store: &InMemoryStore,
    self_guid: u64,
    map_id: u32,
    attacking_target: Option<u64>,
    ranged_repeat: bool,
) -> WorldConn {
    let (mut client, mut server_end) = UnixStream::pair().unwrap();
    let ch = std::thread::spawn(move || client_handshake(&mut client, "TESTER", K));
    let (mut conn, _encrypt) = world_handshake(&mut server_end, store).unwrap().unwrap();
    ch.join().unwrap();
    conn.state = WorldState::InWorld(InWorld {
        self_guid,
        subs: PlayerSubscriptions::empty(),
        session_epoch: 0,
        attacking_target,
        looting_target: None,
        ranged_repeat,
        map_id,
        seam: SeamTracker::new(),
        handoff_in_progress: false,
        pending_handoff_movement: Default::default(),
        last_handoff_attempt: None,
    });
    conn
}

/// #72 slice 2 — the headline: a CONFIRMED seam crossing drives the escrowed transfer and re-homes
/// the session with NO loading screen (no `SMSG_TRANSFER_PENDING`/`SMSG_TRANSFER_ABORTED`/
/// `SMSG_NEW_WORLD`), the socket stays open, and movement sent AFTER the crossing lands on the
/// DESTINATION rather than the shard the player just left. Exercises the real wiring end to end:
/// `forward_movement` -> `handle_seam_crossing` -> `drive_warm_handoff` -> `run_transfer` against
/// two independent `FakeShardDb`s standing in for two databases, then `player_login` +
/// `subscribe_player_events` on the destination.
///
/// Since #326 the crossing is also the acceptance test for the seam NOTICE: with nothing exported
/// (`LYRACORE_SEAM_NOTIFY` unset — this process never sets it), the handoff announces its start and
/// its completion as two System chat lines and sends nothing else. "No loading screen" was always a
/// statement about OPCODES, not about byte silence, so the assertion below pins the exact traffic
/// instead of merely counting it: two `SMSG_MESSAGECHAT`s and then quiet.
#[test]
fn a_confirmed_seam_crossing_drives_a_warm_handoff_with_no_loading_screen() {
    use wow_world_messages::vanilla::{
        MSG_MOVE_HEARTBEAT_Client, MovementInfo, MovementInfo_MovementFlags, Vector3d,
    };

    let src_db = FakeShardDb::with_character(
        XGUID,
        FakeChar {
            map_id: 0,
            instance_id: 0,
            payload: "warm-handoff".into(),
        },
    );
    let dst_db = FakeShardDb::empty();
    let dst = std::sync::Arc::new(InMemoryStore {
        shard: "dst".into(),
        xdb: Some(dst_db.clone()),
        login_entity: Some(warrior_entity()),
        ..Default::default()
    });
    let store = std::sync::Arc::new(InMemoryStore {
        shard: "src".into(),
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        login_entity: Some(warrior_entity()), // map 0, x=-8949.95, y=-132.493
        xdb: Some(src_db.clone()),
        region_shard_answer: Some("dst".into()),
        named_shards: std::sync::Mutex::new(vec![("dst".into(), dst.clone())]),
        ..Default::default()
    });

    let (mut client, server_end) = UnixStream::pair().unwrap();
    let server_store = store.clone();
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.as_ref()));

    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    // Drain the login sequence + self CREATE_OBJECT2 before driving movement.
    for _ in 0..10 {
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    }

    let beat = |o: f32, x: f32, y: f32, t: u32| MSG_MOVE_HEARTBEAT_Client {
        info: MovementInfo {
            flags: MovementInfo_MovementFlags::empty(),
            timestamp: t,
            position: Vector3d { x, y, z: 83.5312 },
            orientation: o,
            fall_time: 0.0,
        },
    };
    // 1st foreign cell (the login position): `Awaiting`, no drive yet.
    beat(1.0, -8949.95, -132.493, 100)
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    // 2nd CONSECUTIVE foreign cell: confirms and drives the handoff, synchronously, inside this
    // very packet's own dispatch.
    beat(1.2, -8949.95, -132.493 - 600.0, 200)
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    // A THIRD heartbeat, sent AFTER landing — must reach the DESTINATION, never the shard just left.
    beat(1.3, -8949.95, -132.493 - 650.0, 300)
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    // 250ms is generous against a synchronous drive measured at ~17ms in production.
    client
        .set_read_timeout(Some(std::time::Duration::from_millis(250)))
        .unwrap();
    // #326: the two seam notices, in order, and NOTHING else — no loading-screen opcode of any
    // kind. Reading them here is also what proves the default-ON polarity end to end: this test
    // process exports no `LYRACORE_SEAM_NOTIFY`, so an accidental flip back to opt-in shows up as
    // a read timeout rather than as silence nobody checks.
    for expected in ["Crossing the seam from src into dst", "You are now on dst"] {
        match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec) {
            Ok(ServerOpcodeMessage::SMSG_MESSAGECHAT(m)) => {
                assert!(
                    m.message.starts_with(expected),
                    "expected the seam notice {expected:?}, got {:?}",
                    m.message
                );
                assert!(
                    matches!(
                        m.chat_type,
                        wow_world_messages::vanilla::SMSG_MESSAGECHAT_ChatType::System { .. }
                    ),
                    "the seam notice must be a System line, got {:?}",
                    m.chat_type
                );
            }
            Ok(other) => panic!(
                "a warm handoff must send only its two seam notices — no \
                 SMSG_TRANSFER_PENDING/_ABORTED/NEW_WORLD. Got: {other}"
            ),
            Err(e) => panic!("the seam notice {expected:?} never arrived: {e}"),
        }
    }
    let mut probe = [0u8; 1];
    assert!(
        client.read(&mut probe).map(|n| n == 0).unwrap_or(true),
        "after the two seam notices a warm handoff must send NOTHING further on the wire — no \
         SMSG_TRANSFER_PENDING/_ABORTED/NEW_WORLD"
    );

    drop(client);
    server
        .join()
        .unwrap()
        .expect("the session must survive a successful warm handoff");

    assert!(
        !src_db.has(XGUID),
        "the source copy must be gone after a completed handoff"
    );
    assert!(
        dst_db.live(XGUID),
        "the character must be live + durable at the destination"
    );
    assert!(
        src_db.settled() && dst_db.settled(),
        "both escrow ledgers must be empty once the handoff lands"
    );
    assert_eq!(
        store.moves.lock().unwrap().len(),
        2,
        "the two heartbeats BEFORE the crossing (their own submit runs before the seam check even \
         fires) land on the SOURCE"
    );
    assert_eq!(
        dst.moves.lock().unwrap().len(),
        1,
        "the heartbeat sent AFTER the crossing must land on the DESTINATION — lost or replayed onto \
         the source is exactly the AC this proves against"
    );
    assert_eq!(
        dst.subscribed.lock().unwrap().len(),
        1,
        "the destination's AOI/relay subs must be (re)built exactly once, with no batch resend"
    );
    assert_eq!(
        dst.bound_sessions.lock().unwrap().as_slice(),
        &[7],
        "the session's identity must be bound on the destination before player_login/subscribe run there"
    );
}

/// #72 (live defect fix) source-scan tripwire for the ONE line no behavioral test can reach: the
/// mock `WorldStore`'s `subscribe_player_events` returns `PlayerSubscriptions::empty()` — a no-op
/// guard with no live SDK callbacks (see its own doc) — so nothing in this crate's harness can
/// observe the flag's EFFECT (a self-relay actually suppressed). What CAN be pinned is its ORDER:
/// `drive_warm_handoff` must call `iw.subs.set_self_relay_suppressed(true)` on the OLD subs BEFORE
/// the drive reaches `run_transfer`'s cascade delete — the same technique
/// `run_transfer_still_arms_the_injector_from_the_environment` (below) uses for the other line this
/// harness cannot reach. `stdb::subscriptions`'s own `every_self_keyed_relay_consults_the_handoff_
/// suppression_flag_issue_72` covers the OTHER half (every relay actually checks the flag once set).
#[test]
fn drive_warm_handoff_suppresses_the_old_subs_self_relays_before_the_drive() {
    let src = include_str!("mod.rs");
    let suppress_at = src.find("iw.subs.set_self_relay_suppressed(true);").expect(
        "`drive_warm_handoff` no longer suppresses the OLD subs' self-relays before driving the \
         transfer — every item/quest/skill/... row `finish_transfer`'s cascade deletes on the \
         source will relay to the client as a real loss again (the diagnosed #72 defect)",
    );
    let drive_at = src
        .find(
            "let drive = drive_warm_handoff_inner(tx, store, conn, self_guid, map_id, target_db, x, y, z, o);",
        )
        .expect("`drive_warm_handoff`'s call into `drive_warm_handoff_inner` moved or was renamed");
    assert!(
        suppress_at < drive_at,
        "the suppression flag is set AFTER `drive_warm_handoff_inner` runs — by the time \
         `run_transfer`'s cascade delete fires inside it, the OLD subs' relays are still armed, \
         which is the exact live defect this fix exists to close"
    );
    // Exactly once, and never un-set: see the field doc on
    // `PlayerSubscriptions::self_relay_suppressed` for why a second call (e.g. un-suppressing on
    // some path) would be wrong, not merely redundant — there is no scenario in which this
    // instance's relays need to un-mute themselves.
    assert_eq!(
        src.matches("set_self_relay_suppressed(").count(),
        1,
        "`set_self_relay_suppressed` is called more than once in this file — a second call site \
         (e.g. clearing the flag on some path) reopens the relay window this fix closes for \
         whatever happens between the clear and the old subs' teardown"
    );
}

/// #72 slice 2 — a handoff to a shard that is not actually connected (misconfigured, or dropped
/// between the seam menu being drawn and the crossing) fails LOUDLY and ends the session, rather
/// than leaving the player half-moved or silently stuck on the source. `settle_transfer` re-drives
/// whatever is recoverable at the client's next login — the same recovery every other crash point
/// in the escrowed transfer already has.
#[test]
fn a_handoff_to_an_unreachable_shard_fails_loudly_and_ends_the_session() {
    use wow_world_messages::vanilla::{
        MSG_MOVE_HEARTBEAT_Client, MovementInfo, MovementInfo_MovementFlags, Vector3d,
    };

    let store = std::sync::Arc::new(InMemoryStore {
        shard: "src".into(),
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        login_entity: Some(warrior_entity()),
        // Names a region shard that was never wired into `named_shards` — "connected at seam-menu
        // time, gone by the time someone actually crosses" (or simply misconfigured).
        region_shard_answer: Some("ghost-shard".into()),
        ..Default::default()
    });

    let (mut client, server_end) = UnixStream::pair().unwrap();
    let server_store = store.clone();
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.as_ref()));
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    for _ in 0..10 {
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    }

    let beat = |o: f32, x: f32, y: f32, t: u32| MSG_MOVE_HEARTBEAT_Client {
        info: MovementInfo {
            flags: MovementInfo_MovementFlags::empty(),
            timestamp: t,
            position: Vector3d { x, y, z: 83.5312 },
            orientation: o,
            fall_time: 0.0,
        },
    };
    beat(1.0, -8949.95, -132.493, 100)
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    beat(1.2, -8949.95, -132.493 - 600.0, 200)
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    drop(client);
    let err = server.join().unwrap().expect_err(
        "a handoff to an unreachable shard must end the session rather than leave it half-moved",
    );
    assert!(format!("{err:#}").contains("ghost-shard"), "{err:#}");
    assert_eq!(
        store.moves.lock().unwrap().len(),
        2,
        "both heartbeats before the failed drive still land on the source — a failed handoff must \
         not retroactively undo movement that already committed"
    );
}

/// #72 slice 2 — combat guard: a confirmed crossing while `attacking_target` is set must SKIP the
/// drive (no module call, no re-homing) and RE-ARM the tracker, so a player who fights their way
/// across a seam gets a second chance at the very next foreign cell instead of being stuck until
/// they leave the region and re-enter (`Confirmed` would otherwise suppress every further crossing
/// into the same db for the rest of the dwell).
#[test]
fn combat_skips_the_handoff_and_rearms_the_tracker() {
    use wow_world_messages::vanilla::{MovementInfo, MovementInfo_MovementFlags, Vector3d};

    let store = std::sync::Arc::new(InMemoryStore {
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        shard: "src".into(),
        region_shard_answer: Some("dst".into()),
        ..Default::default()
    });
    let mut conn = make_inworld_conn(&store, XGUID, 0, Some(99), false); // mid-swing on guid 99

    // Drive the REAL tracker to a confirmed crossing (two consecutive foreign cells) — not a
    // fabricated `SeamCrossing`, so this exercises the actual hysteresis state `rearm` mutates.
    assert!(seam_check(store.as_ref(), &mut conn, 0.0, 0.0).is_none());
    let crossing = seam_check(store.as_ref(), &mut conn, 0.0, 700.0)
        .expect("confirms on the 2nd foreign cell");

    let (tx, _rx, _depth) = session_channel();
    let info = MovementInfo {
        flags: MovementInfo_MovementFlags::empty(),
        timestamp: 1,
        position: Vector3d {
            x: 0.0,
            y: 700.0,
            z: 0.0,
        },
        orientation: 0.0,
        fall_time: 0.0,
    };
    handle_seam_crossing(&tx, store.as_ref(), &mut conn, &crossing, &info)
        .expect("a combat skip must not error — it is a normal, expected outcome");

    let WorldState::InWorld(iw) = &mut conn.state else {
        panic!("still in world")
    };
    assert!(!iw.handoff_in_progress);
    assert!(
        iw.last_handoff_attempt.is_none(),
        "a combat skip is not an ATTEMPT — it must not consume the cooldown"
    );
    // Proof of rearm: the very NEXT foreign cell reconfirms immediately (one cell, not two).
    assert!(
        iw.seam.check(0.0, 750.0, |_, _| Some("dst".to_string())).is_some(),
        "combat must rearm the tracker, or a fighting player never gets a second chance at the handoff"
    );
}

/// #72 slice 2 — the per-session cooldown: a SECOND confirmed crossing (into a different db) fired
/// immediately after a successful handoff must be skipped (not driven) and the tracker rearmed. The
/// second crossing's target ("dst2") is deliberately NOT wired into `named_shards`, so if the
/// cooldown guard were missing, the drive would run for real, fail loudly at `shard_by_name`, and
/// this test's `expect` on `handle_seam_crossing` would panic — the guard is the only thing that
/// can make this call return `Ok`.
#[test]
fn the_cooldown_skips_a_repeat_handoff_attempt_and_rearms() {
    use wow_world_messages::vanilla::{MovementInfo, MovementInfo_MovementFlags, Vector3d};

    let src_db = FakeShardDb::with_character(
        XGUID,
        FakeChar {
            map_id: 0,
            instance_id: 0,
            payload: "x".into(),
        },
    );
    let dst_db = FakeShardDb::empty();
    let dst = std::sync::Arc::new(InMemoryStore {
        shard: "dst".into(),
        xdb: Some(dst_db.clone()),
        login_entity: Some(warrior_entity()),
        // From the START (an `Arc` field can't be mutated later): once `conn.home` becomes `dst`,
        // ITS answer is what the seam check reads — a third shard, so the second crossing is a
        // genuinely different, otherwise-legitimate confirmation, not a same-destination re-entry.
        region_shard_answer: Some("dst2".into()),
        ..Default::default()
    });
    let store = std::sync::Arc::new(InMemoryStore {
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        shard: "src".into(),
        xdb: Some(src_db.clone()),
        region_shard_answer: Some("dst".into()),
        named_shards: std::sync::Mutex::new(vec![("dst".into(), dst.clone())]),
        ..Default::default()
    });
    let mut conn = make_inworld_conn(&store, XGUID, 0, None, false);
    let (tx, _rx, _depth) = session_channel();

    // First crossing: drives for real and lands on `dst`.
    assert!(seam_check(store.as_ref(), &mut conn, 0.0, 0.0).is_none());
    let crossing1 = seam_check(store.as_ref(), &mut conn, 0.0, 700.0)
        .expect("confirms on the 2nd foreign cell");
    let info1 = MovementInfo {
        flags: MovementInfo_MovementFlags::empty(),
        timestamp: 1,
        position: Vector3d {
            x: 0.0,
            y: 700.0,
            z: 0.0,
        },
        orientation: 0.0,
        fall_time: 0.0,
    };
    handle_seam_crossing(&tx, store.as_ref(), &mut conn, &crossing1, &info1)
        .expect("the first attempt must actually land");
    assert!(
        dst_db.live(XGUID),
        "the first attempt must have landed on dst"
    );

    // Second crossing, into "dst2" — resolved through `dst` now that `conn.home` names it. Well
    // inside the 5s cooldown (this whole test runs in microseconds). Two consecutive foreign cells,
    // same as the first crossing: 1st Awaiting (no crossing), 2nd Confirmed.
    assert!(seam_check(dst.as_ref(), &mut conn, 0.0, 1400.0).is_none());
    let crossing2 =
        seam_check(dst.as_ref(), &mut conn, 0.0, 2100.0).expect("confirms on the 2nd foreign cell");
    let info2 = MovementInfo {
        flags: MovementInfo_MovementFlags::empty(),
        timestamp: 2,
        position: Vector3d {
            x: 0.0,
            y: 2100.0,
            z: 0.0,
        },
        orientation: 0.0,
        fall_time: 0.0,
    };
    handle_seam_crossing(&tx, dst.as_ref(), &mut conn, &crossing2, &info2).expect(
        "the cooldown guard must skip this — if it did not, the drive would try (and fail loudly \
         on) `shard_by_name(\"dst2\")`, which is not wired up",
    );

    let WorldState::InWorld(iw) = &mut conn.state else {
        panic!("still in world")
    };
    assert!(!iw.handoff_in_progress);
    assert!(
        iw.seam
            .check(0.0, 2200.0, |_, _| Some("dst2".to_string()))
            .is_some(),
        "the cooldown skip must rearm the tracker too"
    );
}

/// #72 hot-state audit follow-up: the guard STACK, not either guard alone, under a THRASHING
/// player — someone straddling the seam at heartbeat cadence, not a single clean repeat crossing
/// (`the_cooldown_skips_a_repeat_handoff_attempt_and_rearms` above already covers that one).
///
/// After the first real landing, every following heartbeat below lands on a genuinely NEW grid
/// cell resolving to a foreign db different from the one just confirmed — which `rearm` primes as
/// `Awaiting`, so the tracker's OWN hysteresis reconfirms almost every single one of them (this is
/// deliberate: a hysteresis-only bound would NOT catch this burst, so a test that never lets the
/// tracker reconfirm would prove nothing about the cooldown). The property under test is that the
/// per-session wall-clock cooldown still bounds the whole burst to the ONE attempt that already
/// landed in leg 1 — `last_handoff_attempt` must never advance past it, and every one of the ~8
/// reconfirmations must be skipped (never reach `shard_by_name`, which is not wired for the bogus
/// target name used here — a leaked attempt would `expect`-panic on the resulting `Err`).
///
/// Mutation-check: deleting GUARD 2 (the cooldown block) in `handle_seam_crossing` makes every
/// iteration of the loop below drive for real against `shard_by_name("dst2")`, which `dst`'s store
/// never wires up — the `.expect(...)` inside the loop panics on the first one, failing this test
/// loudly rather than silently passing.
#[test]
fn the_guard_stack_bounds_a_heartbeat_cadence_thrash_to_one_attempt_per_window() {
    use wow_world_messages::vanilla::{MovementInfo, MovementInfo_MovementFlags, Vector3d};

    let src_db = FakeShardDb::with_character(
        XGUID,
        FakeChar {
            map_id: 0,
            instance_id: 0,
            payload: "thrash".into(),
        },
    );
    let dst_db = FakeShardDb::empty();
    let dst = std::sync::Arc::new(InMemoryStore {
        shard: "dst".into(),
        xdb: Some(dst_db.clone()),
        login_entity: Some(warrior_entity()),
        // A SECOND foreign name, deliberately never wired into ANY `named_shards` — if a single
        // reconfirmation in the thrash loop below ever reaches `drive_warm_handoff`, it fails
        // loudly resolving this name, which is exactly the "attempt storm" outcome this test exists
        // to rule out.
        region_shard_answer: Some("dst2".into()),
        ..Default::default()
    });
    let store = std::sync::Arc::new(InMemoryStore {
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        shard: "src".into(),
        xdb: Some(src_db.clone()),
        region_shard_answer: Some("dst".into()),
        named_shards: std::sync::Mutex::new(vec![("dst".into(), dst.clone())]),
        ..Default::default()
    });
    let mut conn = make_inworld_conn(&store, XGUID, 0, None, false);
    let (tx, _rx, _depth) = session_channel();

    let mk_info = |t: u32, y: f32| MovementInfo {
        flags: MovementInfo_MovementFlags::empty(),
        timestamp: t,
        position: Vector3d { x: 0.0, y, z: 0.0 },
        orientation: 0.0,
        fall_time: 0.0,
    };

    // LEG 1: two consecutive foreign cells against "dst" (wired) — the one real attempt, and it
    // must land.
    assert!(seam_check(store.as_ref(), &mut conn, 0.0, 0.0).is_none());
    let crossing1 = seam_check(store.as_ref(), &mut conn, 0.0, 700.0)
        .expect("confirms on the 2nd foreign cell");
    handle_seam_crossing(
        &tx,
        store.as_ref(),
        &mut conn,
        &crossing1,
        &mk_info(100, 700.0),
    )
    .expect("leg 1 must land for real");
    assert!(dst_db.live(XGUID), "leg 1 must have actually landed on dst");
    let recorded_attempt = {
        let WorldState::InWorld(iw) = &mut conn.state else {
            panic!("still in world")
        };
        iw.last_handoff_attempt
            .expect("leg 1 recorded a real attempt")
    };

    // THE THRASH: ~8 more heartbeats, heartbeat-cadence fast (this whole loop runs in
    // microseconds — the same real-time assumption `the_cooldown_skips_a_repeat_handoff_attempt_
    // and_rearms` already relies on), every one resolving through `dst` to the bogus "dst2". The
    // first reconfirms after 2 cells (a different foreign db than the one just landed on); `rearm`
    // then primes every SUBSEQUENT single cell to reconfirm immediately — the realistic shape of a
    // player oscillating across a seam line at the client's movement-packet cadence.
    let mut confirmed = 0u32;
    let mut y = 700.0f32;
    for step in 0u32..8 {
        y += 100.0;
        let t = 200 + step * 50;
        if let Some(crossing) = seam_check(dst.as_ref(), &mut conn, 0.0, y) {
            confirmed += 1;
            handle_seam_crossing(&tx, dst.as_ref(), &mut conn, &crossing, &mk_info(t, y)).expect(
                "the cooldown guard must skip every reconfirmation in this window — if it did \
                 not, the drive would try (and fail loudly on) shard_by_name(\"dst2\"), which is \
                 not wired up",
            );
        }
        let WorldState::InWorld(iw) = &mut conn.state else {
            panic!("still in world")
        };
        assert_eq!(
            iw.last_handoff_attempt,
            Some(recorded_attempt),
            "step {step}: a SKIPPED reconfirmation must not itself count as a fresh attempt — the \
             cooldown clock must stay anchored to leg 1's attempt, or a thrashing player could \
             extend their own cooldown window into a permanent block on every future real crossing"
        );
    }
    assert!(
        confirmed >= 2,
        "this test is only meaningful if the tracker's OWN hysteresis reconfirmed more than once \
         during the thrash (proving hysteresis ALONE does not bound this burst, only the cooldown \
         does) — got {confirmed} reconfirmations"
    );
    assert!(
        dst_db.live(XGUID),
        "the one real landing from leg 1 must be undisturbed by the thrash"
    );
    assert!(
        src_db.settled() && dst_db.settled(),
        "no half-attempted transfer state may be left behind by a skipped reconfirmation"
    );
}

/// #72 slice 2 — while a handoff is mid-drive, inbound movement must be QUEUED, never submitted to
/// either shard (the source is about to be frozen away; the destination has no subs yet).
#[test]
fn movement_received_while_a_handoff_is_in_progress_is_queued_not_submitted() {
    use wow_world_messages::vanilla::{MovementInfo, MovementInfo_MovementFlags, Vector3d};

    let store = std::sync::Arc::new(InMemoryStore {
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        ..Default::default()
    });
    let mut conn = make_inworld_conn(&store, XGUID, 0, None, false);
    if let WorldState::InWorld(iw) = &mut conn.state {
        iw.handoff_in_progress = true;
    }
    let (tx, _rx, _depth) = session_channel();
    let info = MovementInfo {
        flags: MovementInfo_MovementFlags::empty(),
        timestamp: 42,
        position: Vector3d {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        },
        orientation: 0.5,
        fall_time: 0.0,
    };
    forward_movement(
        &tx,
        store.as_ref(),
        &mut conn,
        lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT,
        &info,
    )
    .unwrap();

    assert!(
        store.moves.lock().unwrap().is_empty(),
        "a packet arriving mid-drive must not be submitted to either shard"
    );
    let WorldState::InWorld(iw) = &conn.state else {
        panic!("still in world")
    };
    assert_eq!(iw.pending_handoff_movement.len(), 1);
    assert_eq!(iw.pending_handoff_movement[0].1.timestamp, 42);
}

/// #72 slice 2 — the queue is bounded and drops the OLDEST entry, not the newest: a position
/// snapshot is only worth anything if it is the freshest one.
#[test]
fn the_handoff_movement_queue_is_bounded_and_drops_the_oldest() {
    use wow_world_messages::vanilla::{MovementInfo, MovementInfo_MovementFlags, Vector3d};

    let store = std::sync::Arc::new(InMemoryStore {
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        ..Default::default()
    });
    let mut conn = make_inworld_conn(&store, XGUID, 0, None, false);
    if let WorldState::InWorld(iw) = &mut conn.state {
        iw.handoff_in_progress = true;
    }
    let (tx, _rx, _depth) = session_channel();
    let extra = 8u32;
    for t in 0..(MAX_PENDING_HANDOFF_MOVEMENT as u32 + extra) {
        let info = MovementInfo {
            flags: MovementInfo_MovementFlags::empty(),
            timestamp: t,
            position: Vector3d {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            orientation: 0.0,
            fall_time: 0.0,
        };
        forward_movement(
            &tx,
            store.as_ref(),
            &mut conn,
            lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT,
            &info,
        )
        .unwrap();
    }
    let WorldState::InWorld(iw) = &conn.state else {
        panic!("still in world")
    };
    assert_eq!(
        iw.pending_handoff_movement.len(),
        MAX_PENDING_HANDOFF_MOVEMENT
    );
    assert_eq!(
        iw.pending_handoff_movement.front().unwrap().1.timestamp,
        extra,
        "the oldest `extra` packets must have been dropped, keeping the newest MAX"
    );
    assert_eq!(
        iw.pending_handoff_movement.back().unwrap().1.timestamp,
        MAX_PENDING_HANDOFF_MOVEMENT as u32 + extra - 1
    );
}

/// #72 slice 2 — a movement packet queued mid-drive must replay onto the DESTINATION, never onto
/// the STALE handle `drive_warm_handoff` was called with. This is the one place a single
/// `dispatch` call spans a home change: `run_world_session`'s own loop re-resolves `conn.home`
/// fresh for every OTHER call, but the replay loop is inside the same call that just changed it —
/// so it has to do that re-resolution by hand (`on_home_shard!`) instead of trusting its own
/// parameter. Queues the packet by hand (pre-populating `pending_handoff_movement` before the
/// drive runs) rather than trying to land a real packet inside the ~17ms synchronous window, which
/// a single-threaded test cannot do — the replay CODE PATH is identical either way.
#[test]
fn queued_movement_replays_onto_the_destination_not_the_stale_source_handle() {
    use wow_world_messages::vanilla::{MovementInfo, MovementInfo_MovementFlags, Vector3d};

    let src_db = FakeShardDb::with_character(
        XGUID,
        FakeChar {
            map_id: 0,
            instance_id: 0,
            payload: "x".into(),
        },
    );
    let dst_db = FakeShardDb::empty();
    let dst = std::sync::Arc::new(InMemoryStore {
        shard: "dst".into(),
        xdb: Some(dst_db.clone()),
        login_entity: Some(warrior_entity()),
        ..Default::default()
    });
    let store = std::sync::Arc::new(InMemoryStore {
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        shard: "src".into(),
        xdb: Some(src_db.clone()),
        named_shards: std::sync::Mutex::new(vec![("dst".into(), dst.clone())]),
        ..Default::default()
    });
    let mut conn = make_inworld_conn(&store, XGUID, 0, None, false);
    if let WorldState::InWorld(iw) = &mut conn.state {
        iw.pending_handoff_movement.push_back((
            lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT,
            MovementInfo {
                flags: MovementInfo_MovementFlags::empty(),
                timestamp: 99,
                position: Vector3d {
                    x: 1.0,
                    y: 2.0,
                    z: 3.0,
                },
                orientation: 0.0,
                fall_time: 0.0,
            },
        ));
    }
    let (tx, _rx, _depth) = session_channel();
    drive_warm_handoff(&tx, store.as_ref(), &mut conn, "dst", 5.0, 6.0, 7.0, 0.0)
        .expect("the drive itself must succeed");

    assert!(
        store.moves.lock().unwrap().is_empty(),
        "the queued packet must NOT be replayed onto the shard the player just left"
    );
    assert_eq!(
        dst.moves.lock().unwrap().len(),
        1,
        "the queued packet must replay onto the DESTINATION exactly once"
    );
    assert_eq!(
        dst.moves.lock().unwrap()[0].5,
        99,
        "the replayed packet must be the one that was queued"
    );
}

/// Issue #39 defect 2, the ACTUAL hang. `teleport_player` despawns the live entity the instant the
/// portal's reducer commits, but the client keeps sending movement until `SMSG_TRANSFER_PENDING`
/// reaches it — so every cross-map port has a window in which packets land on an entity that no
/// longer exists, and the module answers "mover not in world". That answer used to propagate as a
/// session-fatal desync and close the socket **while the client was on its loading screen**: no
/// `MSG_MOVE_WORLDPORT_ACK` ever came back, the escrowed transfer that runs on that ack never ran,
/// and the player hung forever. The window is milliseconds for an ordinary teleport and hundreds of
/// milliseconds for a dungeon entry that spawns a 200-creature population — which is exactly why one
/// player transferred fine and the party member behind her did not.
#[test]
fn a_movement_packet_for_a_despawned_entity_never_kills_the_session() {
    use wow_world_messages::vanilla::{
        MSG_MOVE_HEARTBEAT_Client, MSG_MOVE_START_FORWARD_Client, MovementInfo,
        MovementInfo_MovementFlags, Vector3d,
    };
    let calls: ShardCallLog = Default::default();
    let store = std::sync::Arc::new(InMemoryStore {
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        calls: calls.clone(),
        movement_error: Some("mover not in world".into()),
        ..Default::default()
    });

    let (mut client, server_end) = UnixStream::pair().unwrap();
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
    // Two packets: the second only reaches the module if the first did not end the session. It is a
    // STATE TRANSITION (never coalesced, work-item 231 rule 1) so it forwards immediately.
    beat(1)
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
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
        2,
        "the session must keep serving packets after the desynced one is dropped"
    );
}

/// The other side of the same gate: only a DESYNC is swallowed. A transport/reducer failure that
/// means something else is still session-fatal, so the swallow above cannot quietly grow into
/// "movement errors don't matter".
#[test]
fn a_movement_failure_that_is_not_a_desync_is_still_session_fatal() {
    use wow_world_messages::vanilla::{
        MSG_MOVE_HEARTBEAT_Client, MovementInfo, MovementInfo_MovementFlags, Vector3d,
    };
    let store = std::sync::Arc::new(InMemoryStore {
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        movement_error: Some("timed out after 10s".into()),
        ..Default::default()
    });
    let (mut client, server_end) = UnixStream::pair().unwrap();
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

/// The OTHER half of #39's movement gate, and the reason it is bounded rather than unconditional.
/// A desync means the player's entity is gone for good (a schema-change publish tore down the
/// coordinator subscription, or the row was deleted out from under this socket) — `is_desync_error`
/// exists precisely because "no further action can EVER be served on this session", and the cure is
/// a clean disconnect so the client relogs and re-materialises from durable state. Movement is by
/// far the highest-frequency desync detector (~10 packets/s from any moving client), so swallowing
/// it outright trades the loading-screen hang for a WORSE hang: a player walking around a frozen
/// world forever, invisible to peers, never disconnected, with no error — unless they happen to
/// swing at something. The port window is a handful of packets (the client stops sending the moment
/// `SMSG_TRANSFER_PENDING` puts it on the loading screen), so tolerance must END.
#[test]
fn a_movement_desync_that_never_heals_still_ends_the_session() {
    use wow_world_messages::vanilla::{
        MSG_MOVE_START_FORWARD_Client, MSG_MOVE_STOP_Client, MovementInfo,
        MovementInfo_MovementFlags, Vector3d,
    };
    let store = std::sync::Arc::new(InMemoryStore {
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        // The entity is gone and is NEVER coming back — not a teleport tail, a real desync.
        movement_error: Some("mover not in world".into()),
        ..Default::default()
    });
    let (mut client, server_end) = UnixStream::pair().unwrap();
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
         is the hang #39 is about, with no loading screen to blame it on",
    );
    assert!(format!("{err:#}").contains("not in world"), "{err:#}");
}

/// Issue #39 AC#4. A transfer that cannot be driven at `MSG_MOVE_WORLDPORT_ACK` used to close the
/// socket with the client mid-load — an infinite loading bar, no message, no recourse. The entry
/// must FAIL LOUDLY instead: the client is told the transfer is off (`SMSG_TRANSFER_ABORTED`,
/// naming the map it is loading) before the session ends.
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
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        characters: vec![],
        login_entity: Some(warrior_entity()),
        xdb: Some(xdb),
        settle_error: Some("instances shard unreachable".into()),
        settle_ok_calls: 1, // the LOGIN routes fine; the world-port's settle is the one that fails
        ..Default::default()
    });

    let (mut client, server_end) = UnixStream::pair().unwrap();
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

    let aborted = ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec)
        .expect("the client must receive SOMETHING — silence here is the hang this fixes");
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

/// #39 AC#4, the half the abort above misses: ROUTING is not the only step of a world-port that can
/// fail. `route_home` may succeed (the transfer runs, the character is now durably on the
/// destination shard) and the RE-ENTRY behind it still fail — `player_login` refused by the
/// stranding guard, a subscription that could not be registered, a login batch that would not
/// build. The client is on exactly the same loading screen, entered for exactly the same reason,
/// and a bare `?` there closes the socket mid-load: the same infinite loading bar, from a window
/// that is WIDER than the routing one (it spans the whole world entry). The abort must cover the
/// whole world-port, not just its first step.
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
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        characters: vec![],
        login_entity: Some(warrior_entity()),
        xdb: Some(xdb),
        // Routing succeeds; the world entry on the far side is what fails.
        worldport_login_error: Some("character 1 is stranded on map 36".into()),
        ..Default::default()
    });

    let (mut client, server_end) = UnixStream::pair().unwrap();
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
         here is the infinite loading bar #39 exists to kill",
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

/// Work-item 231, the HARD requirement: a peer-visibility test proving state-transition packets
/// are NEVER delayed by the coalescer, driven through the real `dispatch` loop (not just the pure
/// `CoalesceState` unit tests in `coalesce.rs`). Sequence: run-start, two same-vector heartbeats
/// (held/superseded — the sub-yard intermediate at x=5.0 must drop ENTIRELY, rule 3), a turn
/// (state change — must flush the pending heartbeat FIRST, then itself, undelayed), then a stop
/// (state change — forwards immediately with no pending left to flush).
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

/// Work-item 231, rule 2 (the robust flush-on-any-other-opcode): a non-movement CMSG must see the
/// module's CURRENT position, so a pending coalesced heartbeat is flushed BEFORE any other opcode
/// is dispatched — proven here by driving a real non-movement opcode (`CMSG_QUESTGIVER_STATUS_QUERY`,
/// the same no-op sentinel other tests in this file use) and observing that only the LATEST held
/// heartbeat lands in `store.moves`, never the two superseded intermediates — distinguishing "the
/// query's dispatch flushed the pending packet" from "coalescing wasn't happening at all" (which
/// would forward all three heartbeats individually, a different, observably larger count). No
/// `store.moves` peek happens before the round-trip read below completes — reading the client's
/// reply is itself the synchronization point proving the server-side dispatch (flush, then the
/// query handler) already ran, so there's no race against the reader thread's async processing.
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
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        login_entity: Some(warrior_entity()),
        ..Default::default()
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
/// drain the 10-message login sequence (LYRACORE_QUEST_LOG off in tests → no quest-log update appended).
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
    let (mut client, server_end) = UnixStream::pair().unwrap();
    // The login sequence ends with the quest-log VALUES packet IFF the player has quests (mirrors
    // `send_quest_log`'s skip-when-empty). Checked before `store` is moved into the server thread.
    let has_quest_log = store.player_quest_log(guid).is_ok_and(|s| !s.is_empty());
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
    for _ in 0..10 {
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

/// **Task B2 — the writer's half of the egress depth counter, at its call site.** The whole shed
/// mechanism rests on the depth going back DOWN as packets reach the socket: without the writer's
/// `fetch_sub` the counter only ever climbs, every session crosses `EGRESS_SHED_DEPTH` after ~512
/// packets of ordinary play, and peer movement is then shed forever. That decrement lives inside the
/// spawned writer loop, so this drives a real `run_world_session` and reads the real counter through
/// the depth handle the fake store captures.
///
/// Deterministic without a sleep: the writer decrements BEFORE it writes, so anything the client has
/// finished reading is already accounted for, and `enter_world` has read the whole login sequence by
/// the time it returns. (Depth counts ITEMS, and the login sequence rides one `Outbound::Batch`, so
/// the pre-drain peak here is small — the mutation this catches turns 0 into 1.)
///
/// What it does NOT catch: an increment/decrement pair that is wrong by the same amount in both
/// places (both deleted reads as 0 too) — `the_egress_depth_counts_queued_items_and_rolls_back_a_\
/// failed_send_b2` in `stdb::subscriptions` pins the increment side on its own.
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
    // Work-item 066: CMSG_GROUP_INVITE "Buddy" resolves the name, calls the store, and echoes
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
    // Work-item 130: CMSG_ADD_FRIEND "Buddy" -> SMSG_FRIEND_STATUS AddedOnline (guid 2, resolved by
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

/// `resolve_add_contact`'s error-string mapping (work-item 130), driven end-to-end through the
/// existing `trade_error` mock: `add_friend`/`add_ignore` return the module's rejection text and the
/// gateway must translate each needle into its `FriendResult`, distinctly for the friend vs ignore list.
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

/// `resolve_del_contact`'s Err path: removing a guid never on the list maps to NotFound (friend) /
/// IgnoreNotFound (ignore) — distinctly from the Ok(Removed)/Ok(IgnoreRemoved) path already covered
/// by `add_friend_by_name_then_friend_list_carries_online_presence`.
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

// ── Inspect (work-item 137) ──────────────────────────────────────────────────────────────────────

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

// ── Vendor / buy-failed (work item 069) ─────────────────────────────────────────────────────────

#[test]
fn buy_item_err_sends_smsg_buy_failed() {
    // When `buy_item` returns Err (e.g. "not enough money"), the gateway must send SMSG_BUY_FAILED
    // with the matching BuyResult code so the player gets an on-screen error (work item 069).
    let store = std::sync::Arc::new(InMemoryStore {
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        login_entity: Some(warrior_entity()),
        trade_error: Some("not enough money to buy that item".into()),
        ..Default::default()
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

// ── Inventory change failure (work item 070) ─────────────────────────────────────────────────────

#[test]
fn equip_item_err_sends_smsg_inventory_change_failure() {
    // When `equip_item` returns Err (e.g. item requires higher level / wrong class), the gateway
    // must send SMSG_INVENTORY_CHANGE_FAILURE so the client displays the error sound/popup instead
    // of silently snapping the item back (work item 070).
    let store = std::sync::Arc::new(InMemoryStore {
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        login_entity: Some(warrior_entity()),
        trade_error: Some("required level not met".into()),
        ..Default::default()
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
    drop(client);
    server.join().unwrap();
}

// ── Item-starts-quest (work-item 194) ───────────────────────────────────────────────────────────

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
    // The pre-194 baseline: an ordinary item (no start_quest fixture) still goes through use_item.
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

// ── Quest sharing (work-item 194) ───────────────────────────────────────────────────────────────

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
// Logout gate tests (work item #077)
// ===========================================================================

#[test]
fn logout_while_out_of_combat_succeeds() {
    // combat_until_ms=0 (default, never in combat) → CMSG_LOGOUT_REQUEST must reply
    // Success/Instant + LOGOUT_COMPLETE and the logout() store reducer must be called.
    let store = std::sync::Arc::new(InMemoryStore {
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        login_entity: Some(warrior_entity()),
        ..Default::default()
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
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        login_entity: Some(warrior_entity()),
        combat_until_ms: u64::MAX, // always in combat
        ..Default::default()
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
    // work-item 029: CMSG_PLAYED_TIME -> SMSG_PLAYED_TIME. The character row carries a durable
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
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        login_entity: Some(warrior_entity()),
        characters: vec![codec::CharacterView {
            guid: 1,
            name: "Tester".into(),
            played_total_secs: durable_secs,
            session_start_micros,
            ..Default::default()
        }],
        ..Default::default()
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
//  Work-item 179: the deferred handler-level tests — CMSG_CAST_SPELL routing, quest instant
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

/// `SpellCastTargets` carrying a DEST_LOCATION (a ground-targeted click — Flamestrike/Blizzard). [118 p2]
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

// ── CMSG_CAST_SPELL routing (instant-cast ordering [083], Auto Shot intercept #10, enchant [094]) ──

#[test]
fn instant_cast_sends_start_then_raw_cast_result_ok_then_go_and_threads_the_target() {
    // [083] root-cause client-wedge fix: an INSTANT cast must emit START(0) → raw CAST_RESULT(OK,
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
fn auto_shot_intercept_starts_the_ranged_attack_instead_of_casting() {
    // #10/097 vanilla shape: Auto Shot (75) and wand Shoot (5019) are auto-repeat ranged attacks —
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
        client
            .set_read_timeout(Some(std::time::Duration::from_millis(250)))
            .unwrap();
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
    // [094] an ITEM-target cast whose spell routes as Enchant(id): item guid → bag slot →
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

// ── Quest instant routing (CMSG_QUESTGIVER_HELLO, work-item 112) ────────────────────────────────

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

// ── Loot window state machine (slices 3/4) ──────────────────────────────────────────────────────

#[test]
fn loot_opens_the_window_and_loot_money_drives_the_tracked_guid() {
    // CMSG_LOOT arms looting_target and replies the RAW loot window (guid + money in the body);
    // CMSG_LOOT_MONEY (which carries NO guid) must then hit the TRACKED corpse. Work-item 221: a
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
    // amount == 0: the same no-notify contract as any solo loot (work-item 221) — CLEAR_MONEY still
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

// ── Group loot methods (work-item 187 slices 1-4) ───────────────────────────────────────────────

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

// ── Per-viewer quest loot (work-item 187 slice 0) ───────────────────────────────────────────────
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
    // "Both have it -> both loot one each" (the work item's done-when line): the SAME shared corpse,
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
    // quest-only rows at all — the common, pre-187 case) sees an empty window, exactly the existing
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
    let (mut client, server_end) = UnixStream::pair().unwrap();
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
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(80),
        gossip_list_id: codec::GOSSIP_OPTION_VENDOR,
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
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(81),
        gossip_list_id: codec::gossip_option_innkeeper(false),
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

// --- work-item 217: imported gossip menu options + multi-slot npc_text ---------------------------

/// A shorthand imported option builder for the 217 mock tests.
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
    s.quest_log = vec![(60, false)]; // taken, not yet turned in
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
    // Work-item 223: a Say line starting with '.' diverts to gm_command BEFORE send_chat — never a
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
    // Work-item 223: a rejected dot-command (bad gm_level, unknown command, bad args) is relayed back
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
    // Work-item 223: the client's ack to our `.speed`-triggered SMSG_FORCE_RUN_SPEED_CHANGE must be
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
    // A READ DEADLINE (#22, whisper slice). This test is the only witness that a REFUSED whisper
    // answers at all, and the mutation it pins — dropping the reply from the dispatch arm — made it
    // block forever on a packet that will never come instead of failing. A hang is neither a pass nor
    // a fail (two of PR #49's mutations did exactly this); `no_hang`'s lesson, applied at the socket.
    client
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
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
    // Work-item 199: a grouped caller's `/p` reaches the module's `party_chat` reducer with the
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
    // Work-item 199: the module's "not in a group" rejection maps to the SAME
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
    // The world-side half of #42: when release_session says a newer login superseded this socket,
    // leave_world must NOT call logout (deleting the entity would vanish the LIVE player).
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

// ===========================================================================================
//  #447 — the per-account connection release (the fd/thread leak that ends the process).
//
//  Every distinct account's `PlayerConn` costs a websocket fd + an SDK pump OS thread, and until
//  now nothing ever released one: `accept(2)` returns EMFILE when the fd table runs out and both
//  accept loops propagate it into `main`, so the gateway exits after N sessions where N is a pure
//  function of `ulimit -n`. Releasing is only safe once NO socket for the account remains — these
//  tests pin both halves of that (does release; does NOT release early).
// ===========================================================================================

/// Accounts whose cached per-account connection the store has released so far, in order.
fn released(store: &std::sync::Arc<InMemoryStore>) -> Vec<u64> {
    store.released_conns.lock().unwrap().clone()
}

#[test]
fn the_last_socket_for_an_account_releases_its_cached_connection() {
    // The leak itself: one session, opened and torn down, must reclaim the account's connection.
    let store = std::sync::Arc::new(quest_store());
    let (client, _enc, _dec, server) = enter_world(store.clone(), 1);
    assert!(
        released(&store).is_empty(),
        "nothing may be released while the session is live"
    );
    drop(client);
    server.join().unwrap();
    assert_eq!(
        released(&store),
        vec![7],
        "the account's last socket must release its cached per-account connection"
    );
}

#[test]
fn a_reconnect_racing_a_teardown_keeps_the_new_sessions_connection() {
    // THE danger case. Socket B re-logs on the same account while socket A is still tearing down;
    // both share ONE cached `PlayerConn`. Releasing on A's teardown would cut the LIVE player's
    // link — worse than the leak. A's teardown must therefore release nothing, and B must still be
    // servable afterwards.
    let store = std::sync::Arc::new(quest_store());
    let (client_a, _a_enc, _a_dec, server_a) = enter_world(store.clone(), 1);
    let (mut client_b, mut b_enc, mut b_dec, server_b) = enter_world(store.clone(), 1);

    // A goes away (the client vanished / the socket reset).
    drop(client_a);
    server_a.join().unwrap();
    assert!(
        released(&store).is_empty(),
        "A's teardown must NOT release the connection B is still using"
    );

    // B is genuinely still alive on the far side of A's teardown — served over the connection that
    // would have been closed. (`leave_world` back to character select first, so the char enum is
    // dispatched on the realm/default handle exactly as production does.)
    CMSG_LOGOUT_REQUEST {}
        .write_encrypted_client(&mut client_b, &mut b_enc)
        .unwrap();
    ServerOpcodeMessage::read_encrypted(&mut client_b, &mut b_dec).unwrap(); // SMSG_LOGOUT_RESPONSE
    ServerOpcodeMessage::read_encrypted(&mut client_b, &mut b_dec).unwrap(); // SMSG_LOGOUT_COMPLETE
    CMSG_CHAR_ENUM {}
        .write_encrypted_client(&mut client_b, &mut b_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client_b, &mut b_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_ENUM(_) => {}
        other => panic!("B must still be servable after A's teardown, got {other}"),
    }
    assert!(
        released(&store).is_empty(),
        "still nothing released while B is live"
    );

    // Only when B — the last socket — goes does the connection get reclaimed, exactly once.
    drop(client_b);
    server_b.join().unwrap();
    assert_eq!(
        released(&store),
        vec![7],
        "the LAST socket releases the connection, and only it"
    );
}

#[test]
fn a_stale_epoch_teardown_still_releases_when_it_is_the_last_socket() {
    // The release gate is the SOCKET count, not the #42 entity epoch. A socket whose epoch was
    // superseded (so `leave_world` correctly skips `logout`) is still the last socket here, and
    // its connection must still be reclaimed — otherwise every superseded session leaks, which is
    // precisely the mass-churn shape of #447.
    let mut s = quest_store();
    s.stale_session = true;
    let store = std::sync::Arc::new(s);
    let (client, _enc, _dec, server) = enter_world(store.clone(), 1);
    drop(client);
    server.join().unwrap();
    assert!(
        !store
            .logout_called
            .load(std::sync::atomic::Ordering::SeqCst),
        "precondition: a superseded epoch must not delete the newer session's entity"
    );
    assert_eq!(
        released(&store),
        vec![7],
        "a superseded session is still a socket, and its teardown is still a release"
    );
}

// ===========================================================================================
//  Multi-shard routing (#17) — the routing half of Phase A of the elastic-sharding spec (#12).
//  AC#4: reducer calls and subscriptions never target a shard other than the player's home shard.
//  The `InMemoryStore` pair below stands for two DATABASES sharing one ordered call log, so a test
//  can read off exactly which database served every player-scoped call of a whole live session.
// ===========================================================================================

type ShardCallLog = std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>;

/// A two-database topology: `world` (the default handle the listener hands every session — where
/// accounts, sessions, and the character list live) and `instances` (the shard that owns this
/// character's location, i.e. what `home_shard` resolves to). Both write to one shared call log.
fn sharded_stores() -> (std::sync::Arc<InMemoryStore>, ShardCallLog) {
    let calls: ShardCallLog = Default::default();
    // The character's post-world-port entity, for the re-entry test below.
    let mut ported = warrior_entity();
    ported.map_id = 1;
    let home = std::sync::Arc::new(InMemoryStore {
        shard: "instances".into(),
        calls: calls.clone(),
        login_entity: Some(warrior_entity()),
        worldport_entity: Some(ported),
        ..Default::default()
    });
    let world = std::sync::Arc::new(InMemoryStore {
        shard: "world".into(),
        calls: calls.clone(),
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        characters: vec![codec::CharacterView {
            guid: 1,
            name: "Tester".into(),
            race: 1,
            class: 1,
            level: 1,
            ..Default::default()
        }],
        login_entity: Some(warrior_entity()),
        home: Some(home),
        ..Default::default()
    });
    (world, calls)
}

/// One heartbeat at a fixed position — the movement half of the routed traffic.
fn heartbeat(timestamp: u32) -> wow_world_messages::vanilla::MSG_MOVE_HEARTBEAT_Client {
    use wow_world_messages::vanilla::{MovementInfo, MovementInfo_MovementFlags, Vector3d};
    wow_world_messages::vanilla::MSG_MOVE_HEARTBEAT_Client {
        info: MovementInfo {
            flags: MovementInfo_MovementFlags::empty(),
            timestamp,
            position: Vector3d {
                x: -8950.0,
                y: -130.0,
                z: 83.0,
            },
            orientation: 1.5,
            fall_time: 0.0,
        },
    }
}

/// Drive a full session (char-select → login → movement → an attack → disconnect) and return the
/// ordered `(shard, call)` log.
fn drive_routed_session(
    store: std::sync::Arc<InMemoryStore>,
    calls: ShardCallLog,
) -> Vec<(String, String)> {
    let (mut client, server_end) = UnixStream::pair().unwrap();
    let server_store = store;
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.as_ref()).unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);

    // Character select — REALM-scoped, before any character (and therefore any shard) is chosen.
    CMSG_CHAR_ENUM {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();

    // Enter the world: this is where the session pins itself to the character's home shard.
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    for _ in 0..10 {
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    }

    // In-world traffic: a movement heartbeat (flushed by the following non-movement opcode) and a
    // melee swing — one subscription-driven path and one reducer path.
    heartbeat(100)
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    CMSG_ATTACKSWING {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let _ = ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec);

    drop(client); // EOF → teardown runs `logout`
    server.join().unwrap();
    let log = calls.lock().unwrap().clone();
    log
}

#[test]
fn every_player_scoped_call_after_login_targets_the_home_shard_only() {
    // AC#4 (#17): once the session resolves the character's home shard, EVERY reducer call and the
    // per-player subscription run against that database — the login itself, the AOI/relay
    // subscription, movement, combat, and the teardown logout. Nothing leaks back to the default.
    let (store, calls) = sharded_stores();
    let log = drive_routed_session(store, calls);

    assert_eq!(
        log.first().map(|(s, c)| (s.as_str(), c.as_str())),
        Some(("world", "characters")),
        "character select is realm-scoped and must stay on the default database: {log:?}"
    );
    let after_login = &log[1..];
    assert!(
        after_login.iter().all(|(shard, _)| shard == "instances"),
        "no call may target a shard other than the player's home shard: {log:?}"
    );
    for expected in [
        "player_login",
        "subscribe_player_events", // the per-player connection + AOI subscriptions
        "movement_update",
        "start_attack",
        "logout",
    ] {
        assert!(
            log.iter()
                .any(|(shard, call)| shard == "instances" && call == expected),
            "{expected} must have run on the home shard: {log:?}"
        );
    }
}

#[test]
fn a_single_entry_shard_map_never_routes_and_keeps_every_call_on_the_one_database() {
    // The safety property (#17): with no second shard to resolve to — which is what a single-entry
    // (default/unconfigured) shard map always answers — the session never swaps handles, so the
    // whole flow is served by the database the listener handed it, byte-identically to before.
    let (store, calls) = sharded_stores();
    let single = std::sync::Arc::new(InMemoryStore {
        shard: "world".into(),
        calls: calls.clone(),
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        characters: store.characters.clone(),
        login_entity: Some(warrior_entity()),
        home: None, // ← a single-entry shard map: "you are already on the right shard"
        ..Default::default()
    });
    let log = drive_routed_session(single, calls);

    assert!(
        log.iter().all(|(shard, _)| shard == "world"),
        "an unrouted session must never leave its own database: {log:?}"
    );
    for expected in [
        "characters",
        "player_login",
        "subscribe_player_events",
        "movement_update",
        "logout",
    ] {
        assert!(
            log.iter().any(|(_, call)| call == expected),
            "{expected} missing: {log:?}"
        );
    }
}

#[test]
fn a_region_assignment_flip_re_routes_the_next_entrant_and_leaves_the_resident_alone() {
    // #23 AC#2 + AC#4, at the session level. `pool-b` stands for the shard a flipped region was
    // just assigned to; the mock swaps its answer between the two logins the way an operator's
    // epoch bump does.
    //
    // TWO claims are being pinned here, and they are different claims:
    //   1. NEW ENTRANTS follow the flip — the second session's login, subscription, movement,
    //      combat and logout all run on `pool-b`.
    //   2. RESIDENTS DO NOT — the first session's traffic stays on `instances` for its whole life,
    //      because routing is resolved once per world ENTRY and the pin is never revisited. This
    //      ticket moves nobody; live migration is warm handoff, a later ticket.
    let calls: ShardCallLog = Default::default();
    let resolutions: std::sync::Arc<std::sync::atomic::AtomicUsize> = Default::default();
    let instances = std::sync::Arc::new(InMemoryStore {
        shard: "instances".into(),
        calls: calls.clone(),
        home_shard_calls: resolutions.clone(),
        login_entity: Some(warrior_entity()),
        ..Default::default()
    });
    let pool_b = std::sync::Arc::new(InMemoryStore {
        shard: "pool-b".into(),
        calls: calls.clone(),
        home_shard_calls: resolutions.clone(),
        login_entity: Some(warrior_entity()),
        ..Default::default()
    });
    let world = std::sync::Arc::new(InMemoryStore {
        shard: "world".into(),
        calls: calls.clone(),
        home_shard_calls: resolutions.clone(),
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        characters: vec![codec::CharacterView {
            guid: 1,
            name: "Tester".into(),
            race: 1,
            class: 1,
            level: 1,
            ..Default::default()
        }],
        login_entity: Some(warrior_entity()),
        home: Some(instances),
        home_after_flip: Some(pool_b), // the operator flips the region between the two sessions
        ..Default::default()
    });

    // Session 1 — the RESIDENT. Enters before the flip.
    let resident = drive_routed_session(world.clone(), calls.clone());
    assert!(
        resident
            .iter()
            .skip(1)
            .all(|(shard, _)| shard == "instances"),
        "the resident's whole session must stay on the shard it entered on: {resident:?}"
    );
    assert!(
        !resident.iter().any(|(shard, _)| shard == "pool-b"),
        "no resident call may follow the flip — this ticket re-routes NEW ENTRANTS ONLY: {resident:?}"
    );
    assert_eq!(
        resolutions.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "routing is resolved exactly once per world entry, on ANY handle — a mid-session \
         re-resolution is the warm-handoff machinery this ticket deliberately does not build"
    );

    // Session 2 — the NEXT ENTRANT. Same stores, same character, post-flip.
    calls.lock().unwrap().clear();
    let entrant = drive_routed_session(world, calls.clone());
    assert!(
        entrant.iter().skip(1).all(|(shard, _)| shard == "pool-b"),
        "the next entrant must land on the flipped region's shard: {entrant:?}"
    );
    for expected in [
        "player_login",
        "subscribe_player_events",
        "movement_update",
        "logout",
    ] {
        assert!(
            entrant
                .iter()
                .any(|(shard, call)| shard == "pool-b" && call == expected),
            "{expected} must have run on the flipped shard: {entrant:?}"
        );
    }
    assert_eq!(
        resolutions.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "two world entries, two resolutions — no more, and none from either session's own traffic"
    );
}

#[test]
fn a_world_port_keeps_the_pin_when_the_home_shard_still_owns_the_new_map() {
    // A world-port re-resolves routing (the new map may belong to another shard). When the shard
    // the session is ALREADY on still owns the destination it answers "no swap needed" — which must
    // KEEP the pin, not silently drop the session back to the default database.
    let (store, calls) = sharded_stores();
    let (mut client, server_end) = UnixStream::pair().unwrap();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.as_ref()).unwrap();
    });
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
    for _ in 0..10 {
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    }
    drop(client);
    server.join().unwrap();

    let log = calls.lock().unwrap().clone();
    assert!(
        log.iter().all(|(shard, _)| shard == "instances"),
        "the world-port re-entry must stay on the pinned home shard: {log:?}"
    );
    assert_eq!(
        log.iter().filter(|(_, c)| c == "player_login").count(),
        2,
        "login + world-port re-entry both ran on the home shard: {log:?}"
    );
    assert_eq!(
        log.iter()
            .filter(|(_, c)| c == "subscribe_player_events")
            .count(),
        2,
        "the re-entry re-subscribed on the home shard, not the default one: {log:?}"
    );
}

#[test]
fn a_logout_to_character_select_releases_the_home_shard_pin() {
    // Adversarial review of #17: `leave_world` returns the socket to CharSelect but the session
    // stays open, so the NEXT character-select frames (char enum / create / delete) are dispatched
    // through `on_home_shard!` again. Those are REALM-scoped — `game_account` / `game_character`
    // live on the default database — so a pin left over from the character we just logged out of
    // would serve the character list off an instance shard (which, being empty, shows the player
    // no characters at all, and would create/delete rows on the wrong database).
    let (store, calls) = sharded_stores();
    let (mut client, server_end) = UnixStream::pair().unwrap();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.as_ref()).unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);

    // Enter the world (pins to "instances"), then log out back to character select.
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    for _ in 0..10 {
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    }
    CMSG_LOGOUT_REQUEST {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap(); // SMSG_LOGOUT_RESPONSE
    ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap(); // SMSG_LOGOUT_COMPLETE

    // Back at character select: this must be served by the REALM (default) database again.
    CMSG_CHAR_ENUM {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    let enumerated = match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_ENUM(e) => e.characters.len(),
        other => panic!("expected SMSG_CHAR_ENUM, got {other}"),
    };
    drop(client);
    server.join().unwrap();

    let log = calls.lock().unwrap().clone();
    assert_eq!(
        log.iter()
            .filter(|(s, c)| c == "characters" && s == "world")
            .count(),
        1,
        "the post-logout character enum must run on the realm/default database: {log:?}"
    );
    assert_eq!(
        enumerated, 1,
        "the player must still see their characters after a logout"
    );
}

#[test]
fn a_freshly_created_characters_first_login_transfers_off_the_default_shard() {
    // #60 AC#3/#4: `create_character` always writes to the DEFAULT/realm shard, even when the
    // start position routes to a different one under `LYRACORE_SHARD_MAP` — a deliberate decision (see
    // the doc comment on `impl WorldStore for Coordinator::create_character`): create-then-
    // transfer-on-first-login, not create-directly-on-the-owning-shard. That decision rides the
    // SAME `route_home`/`settle_home_shard` machinery (#17/#19/#47) every other login already
    // uses — prove it end to end for a guid the CREATE call ITSELF produced, not one hardcoded
    // independently of it (the tautology the first version of this test was caught on: a bare
    // `CMSG_PLAYER_LOGIN { guid: Guid::new(1) }` logs into `sharded_stores()`'s pre-seeded
    // "Tester" fixture whether or not the preceding CREATE ever ran).
    //
    // So this drives the REAL 1.12 flow instead: CREATE, then re-ENUMERATE — `SMSG_CHAR_CREATE`
    // carries no guid, the client is expected to learn it from the next `CMSG_CHAR_ENUM` — and
    // pick "Newbie"'s guid out of THAT reply before logging in with it. `InMemoryStore::
    // create_character` now actually records the character (see its doc comment) so this guid is
    // genuinely create-produced, and `sharded_stores()`'s single connected `home` shard
    // (`instances`) stands in for a start map that routes off `world`.
    let (store, calls) = sharded_stores();

    let (mut client, server_end) = UnixStream::pair().unwrap();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.as_ref()).unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);

    // The creation itself must land on the default (`world`) shard — the chosen behaviour, not an
    // accident of this test.
    CMSG_CHAR_CREATE {
        name: "Newbie".into(),
        race: Race::Orc,
        class: Class::Warrior,
        gender: Gender::Male,
        skin_color: 0,
        face: 0,
        hair_style: 0,
        hair_color: 0,
        facial_hair: 0,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_CREATE(m) => {
            assert_eq!(m.result, WorldResult::CharCreateSuccess)
        }
        other => panic!("expected SMSG_CHAR_CREATE, got {other}"),
    }

    // Learn the new character's guid the way a real client does: re-enumerate.
    CMSG_CHAR_ENUM {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    let new_guid = match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_ENUM(e) => e
            .characters
            .iter()
            .find(|c| c.name == "Newbie")
            .unwrap_or_else(|| {
                panic!(
                    "the freshly created character never appeared in the char enum — without \
                     this, the login below cannot possibly be testing the character CREATE just \
                     produced. characters: {:?}",
                    e.characters
                )
            })
            .guid
            .guid(),
        other => panic!("expected SMSG_CHAR_ENUM, got {other}"),
    };

    // That same character's FIRST login, in the SAME session, right after creation.
    CMSG_PLAYER_LOGIN {
        guid: Guid::new(new_guid),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    for _ in 0..10 {
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    }
    drop(client);
    server.join().unwrap();

    let log = calls.lock().unwrap().clone();
    for expected in ["player_login", "subscribe_player_events", "logout"] {
        assert!(
            log.iter()
                .any(|(shard, call)| shard == "instances" && call == expected),
            "the freshly created character's FIRST login must drive the transfer onto its start \
             map's owning shard, exactly like every later world entry — {expected} never ran on \
             `instances`: {log:?}"
        );
    }
    assert!(
        !log.iter().any(|(shard, call)| shard == "world"
            && (call == "player_login" || call == "subscribe_player_events")),
        "the first login must not stay on the default shard once routing resolves it elsewhere: \
         {log:?}"
    );
}

// ===========================================================================================
//  Cross-database transfer (#19) — Phase A of the elastic-sharding spec (#12).
//
//  `FakeShardDb` is a faithful re-implementation of the MODULE's escrow guards
//  (`module/src/transfer/mod.rs`'s `plan_begin`/`plan_import`/`plan_finish` + `release_transfer`'s
//  source check), so these tests exercise the one thing the module cannot check for itself: the
//  ORDER the gateway drives two databases in, because each database can only see its own ledger.
//  Two `FakeShardDb`s stand for two SpacetimeDB databases — the same shape #17's `sharded_stores`
//  uses for routing.
//
//  Deliberately NOT a permissive mock: a fake that recorded calls and returned Ok would let every
//  ordering mutation pass, which is the exact coverage gap the #26/#30 reviews kept finding.
// ===========================================================================================

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
/// timeout, so the suite HANGS instead of failing. The #36 review hit exactly that: an ordering
/// mutation of the driver made the gateway suite hang rather than turn a named test red, which is a
/// coverage failure wearing a pass's clothes. A hang must never be a pass (issue #37).
///
/// Deliberate simplification: `try_lock` instead of a watchdog thread per lock — it is one line,
/// it fires instantly, and it names the offending mutex in the panic. The ceiling: it would also
/// fire on genuine cross-thread contention, which these tests do not have (one `FakeShardDb` per
/// test, one thread per test). `no_hang` below is the belt-and-braces net for a hang that is NOT
/// a re-entrant lock (an unbounded retry loop in the driver, say).
fn lk<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.try_lock().expect(
        "re-entrant lock on FakeShardDb: a method is already holding this mutex further up the \
         stack. With `lock()` this would be a DEADLOCK and the suite would HANG instead of failing \
         — see the fn doc on `lk` (issue #37).",
    )
}

/// Run a test body under a wall-clock deadline, so a hang is a FAILURE rather than a CI job that
/// sits at "still running" until someone kills it. Used on the cross-database driver tests — the
/// ones that walk two databases through a multi-step protocol and are therefore the only place in
/// this suite where a wedge could be a loop rather than a lock.
fn no_hang<T: Send + 'static>(secs: u64, f: impl FnOnce() -> T + Send + 'static) -> T {
    let h = std::thread::spawn(f);
    // Poll `is_finished` rather than shipping the result through a channel, so a body that PANICS
    // still propagates its own panic message (via `resume_unwind`) instead of being reported as a
    // hang. The hang is the only thing this wrapper is allowed to rename.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    while !h.is_finished() {
        assert!(
            std::time::Instant::now() < deadline,
            "test body did not finish within {secs}s — treating the hang as a FAILURE (issue #37). \
             A `cargo test` with no per-test timeout reports a wedged test as 'still running', \
             which reads as neither a pass nor a fail; it must read as a fail."
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    match h.join() {
        Ok(v) => v,
        Err(e) => std::panic::resume_unwind(e),
    }
}

/// Realm-core's authoritative party state, as far as the gateway's routing can see it (#22, group
/// slice) — the module's `realm_group_op` rules, modelled at the granularity the ROUTING depends on:
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
    /// #39: every instance id this database actually SPAWNED a population for — one entry per
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

/// A store handle over a `FakeShardDb`, with an optional injected failure at one named step — how
/// "the gateway was killed here" is simulated (that step's transaction never commits).
fn xstore(
    shard: &str,
    db: std::sync::Arc<FakeShardDb>,
    calls: ShardCallLog,
    kill_at: Option<&str>,
) -> std::sync::Arc<InMemoryStore> {
    std::sync::Arc::new(InMemoryStore {
        shard: shard.into(),
        calls,
        xdb: Some(db),
        kill_at: kill_at.map(|s| s.to_string()),
        ..Default::default()
    })
}

const XGUID: u64 = 1;

/// A fresh two-database topology: the character is resident on `world`, and its durable row already
/// names the instance destination — which is what `teleport_player` writes before it despawns the
/// entity for a cross-map hop, i.e. the state the WORLDPORT_ACK handler finds.
#[allow(clippy::type_complexity)]
fn xdb_pair(
    kill_at: Option<&str>,
) -> (
    std::sync::Arc<InMemoryStore>,
    std::sync::Arc<InMemoryStore>,
    std::sync::Arc<FakeShardDb>,
    std::sync::Arc<FakeShardDb>,
    ShardCallLog,
) {
    let calls: ShardCallLog = Default::default();
    let src_db = FakeShardDb::with_character(
        XGUID,
        FakeChar {
            map_id: 36,
            instance_id: 7,
            payload: "gear+spells".into(),
        },
    );
    let dst_db = FakeShardDb::empty();
    let src = xstore("world", src_db.clone(), calls.clone(), kill_at);
    let dst = xstore("instances", dst_db.clone(), calls.clone(), kill_at);
    (src, dst, src_db, dst_db, calls)
}

/// The deadlock the #36 review found, turned into a named failure.
///
/// `FakeShardDb::import_character_blob` used to hold the `in_rows` guard across `db.live()`, which
/// locks `in_rows` again — only the `&&` short-circuit in `has()` kept the happy path alive. When a
/// driver mutation reached that line the gateway suite HUNG instead of turning a test red. Every
/// lock now goes through `lk` (`try_lock`), so the same re-entrancy is an instant, named panic.
///
/// This test asserts the property directly: hold a guard, take the same mutex again, and the
/// process must come back with a failure rather than never coming back at all.
#[test]
fn a_re_entrant_lock_on_the_fake_shard_db_fails_instead_of_hanging() {
    let db = FakeShardDb::with_character(
        XGUID,
        FakeChar {
            map_id: 36,
            instance_id: 7,
            payload: "gear+spells".into(),
        },
    );
    let _held = lk(&db.in_rows);
    // `live()` reads `in_rows`. With `lock()` this call never returns.
    let hit = no_hang(5, {
        let db = db.clone();
        move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| db.live(XGUID)))
                .err()
                .map(|e| {
                    e.downcast_ref::<String>().cloned().unwrap_or_else(|| {
                        e.downcast_ref::<&str>()
                            .map(|s| s.to_string())
                            .unwrap_or_default()
                    })
                })
        }
    });
    let msg = hit.expect(
        "a re-entrant lock on FakeShardDb did not fail — it either succeeded (the fake is no \
         longer mutex-guarded) or it would have hung, and a hang is not a pass (issue #37)",
    );
    assert!(
        msg.contains("re-entrant lock on FakeShardDb"),
        "unexpected panic: {msg}"
    );
}

#[test]
fn a_character_moves_whole_between_two_databases_with_its_rows() {
    // Wall-clock net: a wedged driver must FAIL, not hang the suite (issue #37).
    no_hang(30, || {
        let (src, dst, src_db, dst_db, calls) = xdb_pair(None);
        super::transfer::settle_transfer(src.as_ref(), dst.as_ref(), XGUID)
            .expect("transfer completes");

        assert!(
            !src_db.has(XGUID),
            "the source copy must be destroyed (delete-last)"
        );
        assert!(
            dst_db.live(XGUID),
            "the character must be LIVE at the destination"
        );
        assert_eq!(
            dst_db.get(XGUID).unwrap(),
            FakeChar { map_id: 36, instance_id: 7, payload: "gear+spells".into() },
            "the character-owned ROWS must arrive, not just its identity — a manifest-only blob lands \
             a naked character with no gear, spells or quest log"
        );
        assert!(
            src_db.settled() && dst_db.settled(),
            "no escrow row may outlive a completed transfer"
        );
        assert!(
            lk(&dst_db.instances).contains(&7),
            "the instance must be mirrored onto the destination shard"
        );
        assert_eq!(
            *lk(&src_db.evicted),
            vec![7],
            "the source shard must stop ticking the instance once the run has moved (#19 AC#2)"
        );
        let log = calls.lock().unwrap().clone();
        assert_eq!(
            log,
            vec![
                // The speculative fence-clear on the SOURCE, before anything else: the transfer id is
                // the character guid, so an arrival in-row left here by an earlier hop would make
                // `begin_transfer` replay into a no-op (see
                // `a_second_transfer_of_the_same_character_is_never_swallowed_as_a_replay`). It costs
                // one no-op reducer call on a fresh transfer and is the same cheap release the
                // already-home path makes.
                ("world".to_string(), "release_transfer".to_string()),
                ("world".to_string(), "begin_transfer".to_string()),
                ("instances".to_string(), "ensure_instance".to_string()),
                ("instances".to_string(), "import_character_blob".to_string()),
                ("world".to_string(), "confirm_import".to_string()),
                ("world".to_string(), "finish_transfer".to_string()),
                // #34: realm-core learns where the character settled HERE — after the escrow's own
                // transaction committed, before the arrival copy goes live.
                ("world".to_string(), "publish_shard_index".to_string()),
                ("instances".to_string(), "release_transfer".to_string()),
                ("world".to_string(), "evict_instance_population".to_string()),
            ],
            "the step ORDER is the safety property neither database can check for itself"
        );
    });
}

/// Issue #34 part 1: the realm-core character→shard index is written BY THE TRANSFER, not left for
/// a future login's probe to discover.
///
/// Before this, `set_character_shard` had exactly one caller in the whole gateway — the login
/// self-heal — so a completed cross-database transfer updated the SOURCE database's copy of the
/// index (transactionally, inside `finish_transfer`) and nothing else. The copy `home_shard`
/// actually reads is realm-core's, and it learned about the move at the character's next login, by
/// probing every shard. #20 AC#3 was unmet, and correct only because the probe masked it.
#[test]
fn a_completed_transfer_publishes_the_destination_to_the_realm_core_index() {
    no_hang(30, || {
        let (src, dst, _src_db, _dst_db, _calls) = xdb_pair(None);
        super::transfer::settle_transfer(src.as_ref(), dst.as_ref(), XGUID)
            .expect("transfer completes");
        assert_eq!(
            *src.realm_index.lock().unwrap(),
            vec![(XGUID, 36, 7)],
            "the drive settled the character on map 36 / instance 7 and told realm-core nothing. \
             Without this write the index is only ever corrected by the login self-heal, so every \
             login pays a full shard probe to rediscover a fact the transfer already knew — and #19 \
             and #23 both route on an index that is never true. The published location must be the \
             ESCROW's destination, which is what `finish_transfer` just settled."
        );
    });
}

/// Step 5b publishes the ESCROW OUT-ROW's destination, never the caller's `plan`.
///
/// This is the clause the whole "a replication, not a stale-index generator" argument rests on —
/// the index can only ever name a destination `finish_transfer` actually settled, because it is read
/// from the same row `do_finish` recorded its own receipt from. Every other clause was executed;
/// this one was not, and substituting `plan.dest_*` for `escrow.dest_*` survived the whole suite
/// (adversarial review of PR #46). The two agree on today's call paths, which is exactly why nothing
/// noticed — and `run_transfer` re-reads the escrow precisely because they are not guaranteed to.
#[test]
fn a_resumed_transfer_publishes_the_escrow_destination_not_the_callers_plan() {
    no_hang(30, || {
        let (src, dst, _src_db, _dst_db, _calls) = xdb_pair(None);
        // Open the escrow against the destination the durable row names (map 36 / instance 7).
        let opened = src
            .character_destination(XGUID)
            .expect("the durable row names a destination");
        src.begin_transfer(&opened).expect("the escrow opens");
        // Now drive with a plan naming somewhere ELSE. `begin_transfer` answers `Replay` — the row
        // on disk is the authority and the plan is ignored — so the transfer settles at 36/7.
        let stale = super::transfer::TransferPlan {
            dest_map_id: 0,
            dest_instance_id: 0,
            ..opened
        };
        super::transfer::run_transfer_injected(src.as_ref(), dst.as_ref(), &stale, None)
            .expect("the drive completes against the escrow on disk");
        assert_eq!(
            *src.realm_index.lock().unwrap(),
            vec![(XGUID, 36, 7)],
            "the index was published from the DRIVER'S PLAN instead of the escrow out-row. The plan \
             is whatever the caller happened to hand in; the escrow is what `finish_transfer` just \
             settled and what `do_finish` wrote the source's own receipt from. Publishing the plan \
             makes step 5b able to name a destination the transfer did not go to — the exact \
             stale-index generator #34 exists to rule out — and it does so silently, because on the \
             ordinary call paths the two happen to agree."
        );
    });
}

/// The write is a REQUIRED step of the drive, not a best-effort side call: an unreachable
/// realm-core fails the transfer rather than silently leaving the directory wrong.
#[test]
fn a_transfer_whose_index_publish_fails_does_not_report_success() {
    no_hang(30, || {
        let calls: ShardCallLog = Default::default();
        let src_db = FakeShardDb::with_character(
            XGUID,
            FakeChar {
                map_id: 36,
                instance_id: 7,
                payload: "gear+spells".into(),
            },
        );
        let dst_db = FakeShardDb::empty();
        let src = std::sync::Arc::new(InMemoryStore {
            shard: "world".into(),
            calls: calls.clone(),
            xdb: Some(src_db.clone()),
            publish_error: Some("realm-core database lyracore-realm is not connected".into()),
            ..Default::default()
        });
        let dst = xstore("instances", dst_db.clone(), calls.clone(), None);
        let err = super::transfer::settle_transfer(src.as_ref(), dst.as_ref(), XGUID)
            .expect_err("a failed index publish must fail the drive");
        assert!(
            err.to_string().contains("lyracore-realm"),
            "the failure must name realm-core, not be swallowed: a publish that shrugged off an \
             unreachable index would be exactly the best-effort, independently-committing write \
             #34 exists to remove. Got: {err:#}"
        );

        // …and the failure is RECOVERABLE, which is what makes propagating it safe: the character
        // is already whole at the destination, only fenced, so a fresh driver with a working
        // realm-core finishes the job. Nothing is lost and nothing is duplicated.
        assert!(!src_db.has(XGUID) && dst_db.has(XGUID) && !dst_db.live(XGUID));
        let src2 = xstore("world", src_db.clone(), calls.clone(), None);
        let dst2 = xstore("instances", dst_db.clone(), calls, None);
        super::transfer::settle_transfer(dst2.as_ref(), dst2.as_ref(), XGUID)
            .expect("a fresh driver recovers the fenced arrival copy");
        assert!(dst_db.live(XGUID) && dst_db.settled() && src_db.settled());
        drop(src2);
    });
}

#[test]
fn a_gateway_kill_at_every_transfer_step_recovers_to_exactly_one_whole_copy() {
    // Wall-clock net: a wedged driver must FAIL, not hang the suite (issue #37).
    no_hang(30, || {
        // AC#3, headless half: kill the driver at every step boundary, then let a fresh STATELESS
        // driver re-run — the character ends whole on exactly one shard, every time.
        //
        // Driven off `ABORT_STEPS` itself rather than a literal copy of it. #34 added step 5b
        // (`publish_shard_index`) to `ABORT_STEPS` and to the drive, but the literal list here was
        // not updated — so the one boundary the PR introduced was the one boundary this matrix did
        // not kill at, while the PR reported the matrix as covering it. A hand-copied list of the
        // thing under test can only ever drift in the direction that loses coverage.
        for kill_at in super::transfer::ABORT_STEPS {
            let (src, dst, src_db, dst_db, _) = xdb_pair(Some(kill_at));
            let first = super::transfer::settle_transfer(src.as_ref(), dst.as_ref(), XGUID);
            if kill_at == "evict_instance_population" {
                // The eviction is deliberately best-effort: the character is already whole by then, so
                // failing the player's login over a performance wart would be strictly worse.
                first.expect("an eviction failure must not fail the transfer");
            } else {
                assert!(
                    first.is_err(),
                    "the injected kill at {kill_at} must abort the drive"
                );
            }

            // INVARIANT AT THE CRASH POINT.
            assert!(
                src_db.has(XGUID) || dst_db.has(XGUID),
                "ZERO durable copies after a kill at {kill_at} — the character was lost"
            );
            assert!(
                !(src_db.live(XGUID) && dst_db.live(XGUID)),
                "the character is LIVE on both databases after a kill at {kill_at} — a dupe"
            );

            // A brand-new driver with NO memory of the interrupted attempt: it re-derives the plan from
            // durable state alone (the escrow row, or the character row's own destination), which is
            // the whole of gateway-restart recovery.
            let calls: ShardCallLog = Default::default();
            let src2 = xstore("world", src_db.clone(), calls.clone(), None);
            let dst2 = xstore("instances", dst_db.clone(), calls.clone(), None);
            let holder: &dyn WorldStore = if src_db.has(XGUID) {
                src2.as_ref()
            } else {
                dst2.as_ref()
            };
            super::transfer::settle_transfer(holder, dst2.as_ref(), XGUID)
                .unwrap_or_else(|e| panic!("recovery after a kill at {kill_at} failed: {e:#}"));

            assert!(
                !src_db.has(XGUID),
                "after recovering from a kill at {kill_at} the source copy must be gone"
            );
            assert!(
                dst_db.live(XGUID),
                "after recovering from a kill at {kill_at} the character must be live at the destination"
            );
            assert_eq!(
                dst_db.get(XGUID).unwrap().payload,
                "gear+spells",
                "recovery from a kill at {kill_at} must not lose the character-owned rows"
            );
            assert!(
                src_db.settled() && dst_db.settled(),
                "recovery from a kill at {kill_at} left an escrow row behind"
            );
        }
    });
}

/// `LYRACORE_TRANSFER_ABORT_AFTER=<step>` must let the named step COMMIT and then kill the driver before
/// the next one — that is the only way the live AC#3 matrix can aim at a specific crash boundary in
/// a drive that completes in ~17ms. In a `cargo test` build the injected death is a panic rather
/// than `process::abort()` (see `transfer::die_by_injection`), so it is observable here.
#[test]
fn an_injected_abort_stops_the_driver_after_the_named_step_and_before_the_next() {
    for (i, step) in super::transfer::ABORT_STEPS.iter().enumerate() {
        let (src, dst, src_db, dst_db, calls) = xdb_pair(None);
        let plan = src
            .character_destination(XGUID)
            .expect("the durable row names the destination");
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            super::transfer::run_transfer_injected(src.as_ref(), dst.as_ref(), &plan, Some(step))
        }));
        assert!(
            outcome.is_err(),
            "LYRACORE_TRANSFER_ABORT_AFTER={step} did not kill the driver — it returned {:?}. A step that \
             merely returns (Ok or Err) is a CLEAN exit and reproduces nothing about a kill -9.",
            outcome.as_ref().map(|r| r.is_ok())
        );

        // The step named must have RUN (its transaction committed), and nothing after it may have.
        let log = calls.lock().unwrap().clone();
        let names: Vec<&str> = log.iter().map(|(_, n)| n.as_str()).collect();
        assert_eq!(
            names.last().copied(),
            Some(*step),
            "LYRACORE_TRANSFER_ABORT_AFTER={step} left the shard-call log ending at {:?} — the abort must \
             land AFTER {step} commits, not before it and not after a later step",
            names.last()
        );
        assert_eq!(
            names.len(),
            i + 1,
            "LYRACORE_TRANSFER_ABORT_AFTER={step} drove {} shard calls ({names:?}) — expected exactly the \
             {} steps up to and including {step}",
            names.len(),
            i + 1
        );

        // And the AC#3 invariant the live matrix asserts against the two real databases.
        assert!(
            src_db.has(XGUID) || dst_db.has(XGUID),
            "ZERO durable copies after an injected abort at {step} — the character was lost"
        );
        assert!(
            !(src_db.live(XGUID) && dst_db.live(XGUID)),
            "the character is LIVE on both databases after an injected abort at {step} — a dupe"
        );
    }
}

/// The unconfigured default must be indistinguishable from having no injection point at all: same
/// shard calls, same order, same result. (This repo has shipped three "unconfigured is
/// byte-identical" violations already; this is the guard against a fourth.)
#[test]
fn an_unset_transfer_abort_injection_changes_nothing() {
    assert_eq!(
        std::env::var("LYRACORE_TRANSFER_ABORT_AFTER").ok(),
        None,
        "LYRACORE_TRANSFER_ABORT_AFTER is set in this test process — the fault injector is opt-in and no \
         normal run (or test run) may have it in the environment"
    );

    let (src, dst, src_db, dst_db, calls) = xdb_pair(None);
    let plan = src.character_destination(XGUID).unwrap();
    super::transfer::run_transfer_injected(src.as_ref(), dst.as_ref(), &plan, None)
        .expect("an unconfigured drive must complete exactly as before");

    let injected: Vec<String> = calls
        .lock()
        .unwrap()
        .iter()
        .map(|(_, n)| n.clone())
        .collect();
    assert_eq!(
        injected,
        super::transfer::ABORT_STEPS
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
        "an unconfigured drive must run every step, in order, and nothing else"
    );
    assert!(
        !src_db.has(XGUID) && dst_db.live(XGUID),
        "and land the character whole at the destination"
    );
}

/// Source-scan tripwire for the one line no in-process test can reach: `run_transfer`'s ENV WIRING.
///
/// Both tests above drive `run_transfer_injected` directly — deliberately, so a parallel test
/// runner never has process-global env mutated underneath it — which leaves the wrapper that
/// actually arms the injector in production completely unexercised. Found by mutation during this
/// PR's review: replacing the call's last argument with a literal `None` (the injector still
/// present, still compiled, permanently DISARMED) left all 370 gateway tests GREEN, while
/// `LYRACORE_TRANSFER_ABORT_AFTER` did nothing and every step of the live AC#3 matrix would time out
/// waiting for a death that can no longer happen.
///
/// The unmatched-step warning is pinned here for the same reason: it is the only thing standing
/// between a typo'd step name and a crash matrix that reports PASS for a crash that never fired,
/// and no in-process test asserts a log line.
#[test]
fn run_transfer_still_arms_the_injector_from_the_environment() {
    let src = include_str!("transfer.rs");
    let at = src
        .find("pub fn run_transfer(")
        .expect("`run_transfer` moved");
    let end = src[at..].find("\n}\n").expect("`run_transfer` body");
    let body = &src[at..at + end];
    assert!(
        body.contains("std::env::var(\"LYRACORE_TRANSFER_ABORT_AFTER\")"),
        "`run_transfer` no longer reads LYRACORE_TRANSFER_ABORT_AFTER — the injector is dead in the \
         PRODUCTION build (the tests call `run_transfer_injected` directly and stay green). Body \
         was:\n{body}"
    );
    assert!(
        body.contains("run_transfer_injected(src, dst, plan, abort_after.as_deref())"),
        "`run_transfer` reads the env but no longer THREADS it into `run_transfer_injected` — the \
         read is decorative and every crash point is permanently disarmed. Body was:\n{body}"
    );
    assert!(
        body.contains("ABORT_STEPS.contains(&step)"),
        "`run_transfer` no longer validates the step name against `ABORT_STEPS` — a typo'd \
         LYRACORE_TRANSFER_ABORT_AFTER would then abort NOTHING, silently, and the crash matrix would \
         report a PASS for a crash that never happened. Body was:\n{body}"
    );
}

#[test]
fn the_driver_never_attests_an_import_that_did_not_commit() {
    // `confirm_import` files the in-row that licenses `finish_transfer` to CASCADE-DELETE the
    // source copy. Attesting before the destination copy is durable is the one unrecoverable
    // ordering bug in the protocol — and it is the GATEWAY's to prevent, because the source
    // database cannot see the destination.
    let (src, dst, src_db, dst_db, calls) = xdb_pair(Some("import_character_blob"));
    let err = super::transfer::settle_transfer(src.as_ref(), dst.as_ref(), XGUID)
        .expect_err("a failed import must abort the drive");
    assert!(
        format!("{err:#}").contains("import_character_blob"),
        "{err:#}"
    );

    let log = calls.lock().unwrap().clone();
    assert!(
        !log.iter()
            .any(|(_, c)| c == "confirm_import" || c == "finish_transfer"),
        "nothing may attest or finish after a failed import: {log:?}"
    );
    assert!(
        src_db.has(XGUID),
        "the source copy must survive a failed import"
    );
    assert!(!dst_db.has(XGUID), "no destination copy materialised");
}

#[test]
fn a_transfer_is_never_finished_before_the_destination_copy_is_durable() {
    // The module's own guard, driven through the gateway: `finish_transfer` refuses while the
    // in-row is absent, so even a driver that skipped `confirm_import` cannot destroy the source.
    let (_, _, src_db, dst_db, _) = xdb_pair(None);
    let calls: ShardCallLog = Default::default();
    let src = xstore("world", src_db.clone(), calls.clone(), None);
    let _dst = xstore("instances", dst_db, calls, None);
    let plan = src
        .character_destination(XGUID)
        .expect("the durable row names the destination");
    src.begin_transfer(&plan).expect("escrow opens");

    let err = src
        .finish_transfer(plan.transfer_id)
        .expect_err("finish must refuse");
    assert!(format!("{err:#}").contains("not imported"), "{err:#}");
    assert!(src_db.has(XGUID), "the source copy must still be there");
}

#[test]
fn the_arrival_copy_is_fenced_until_the_source_copy_is_destroyed() {
    // Delete-last, observed from the outside: at every prefix of the drive there is at most ONE
    // live copy, and the destination only goes live after the source copy is gone.
    let (_, _, src_db, dst_db, _) = xdb_pair(None);
    let calls: ShardCallLog = Default::default();
    let src = xstore("world", src_db.clone(), calls.clone(), None);
    let dst = xstore("instances", dst_db.clone(), calls, None);
    let plan = src.character_destination(XGUID).unwrap();

    src.begin_transfer(&plan).unwrap();
    assert!(
        !src_db.live(XGUID) && !dst_db.has(XGUID),
        "frozen on the source, nothing arrived yet"
    );
    let escrow = src.escrowed_transfer(XGUID).unwrap();
    dst.import_character_blob(escrow.transfer_id, &escrow.blob)
        .unwrap();
    assert!(
        dst_db.has(XGUID) && !dst_db.live(XGUID),
        "the arrival copy is durable but FENCED while the source copy still exists"
    );
    assert!(
        src_db.has(XGUID) && !src_db.live(XGUID),
        "and the source copy is durable but frozen"
    );
    src.confirm_import(escrow.transfer_id).unwrap();
    src.finish_transfer(escrow.transfer_id).unwrap();
    assert!(
        !src_db.has(XGUID),
        "the source copy is destroyed BEFORE the release"
    );
    assert!(!dst_db.live(XGUID), "still fenced until the release");
    dst.release_transfer(escrow.transfer_id).unwrap();
    assert!(dst_db.live(XGUID));
}

/// Issue #39 AC#2 + AC#5, the regression this ticket exists for: the SECOND party member walking
/// into a dungeon whose instance the first member already opened.
///
/// Live, this was the case that broke — the first player transferred perfectly, repeatedly, and the
/// party member behind her hung on the loading screen forever with `run_transfer` never entered.
/// The driver half of that is here: two characters whose durable rows name the SAME instance both
/// have to land on the instances shard, in that one instance, with the destination mirroring it
/// once and spawning its population once. A second `ensure_instance` that re-created the dungeon
/// would be a party playing in two copies of Deadmines.
#[test]
fn a_second_party_member_transfers_into_the_instance_the_first_one_opened() {
    no_hang(30, || {
        const LEADER: u64 = XGUID;
        const MEMBER: u64 = XGUID + 1;
        let calls: ShardCallLog = Default::default();
        let src_db = FakeShardDb::with_character(
            LEADER,
            FakeChar {
                map_id: 36,
                instance_id: 7,
                payload: "leader-gear".into(),
            },
        );
        // The second member resolved to the SAME instance id at the portal — that is what the
        // module's party-first resolution (and the `game_instance_binding` each member carries in
        // their blob) is for. Both rows sit on the world shard; both are owed a transfer.
        lk(&src_db.characters).insert(
            MEMBER,
            FakeChar {
                map_id: 36,
                instance_id: 7,
                payload: "member-gear".into(),
            },
        );
        let dst_db = FakeShardDb::empty();
        let src = xstore("world", src_db.clone(), calls.clone(), None);
        let dst = xstore("instances", dst_db.clone(), calls.clone(), None);

        super::transfer::settle_transfer(src.as_ref(), dst.as_ref(), LEADER)
            .expect("the first member transfers");
        super::transfer::settle_transfer(src.as_ref(), dst.as_ref(), MEMBER)
            .expect("the SECOND member must transfer too — this is the entry that hung live");

        for (guid, payload) in [(LEADER, "leader-gear"), (MEMBER, "member-gear")] {
            assert!(
                dst_db.live(guid),
                "guid {guid} must be live on the instances shard"
            );
            assert!(!src_db.has(guid), "guid {guid}'s source copy must be gone");
            let landed = dst_db.get(guid).unwrap();
            assert_eq!(
                landed.instance_id, 7,
                "guid {guid} landed in a DIFFERENT instance — the party is split"
            );
            assert_eq!(
                landed.payload, payload,
                "guid {guid} arrived without its rows"
            );
        }
        assert_eq!(
            *lk(&dst_db.instances),
            std::collections::HashSet::from([7]),
            "exactly one instance may exist on the destination — a second is a second dungeon"
        );
        assert_eq!(
            *lk(&dst_db.populated),
            vec![7],
            "the destination must SPAWN the instance once; the second member joins the live one"
        );
        assert!(
            src_db.settled() && dst_db.settled(),
            "no escrow may outlive either transfer"
        );
        let log = calls.lock().unwrap().clone();
        assert_eq!(
            log.iter()
                .filter(|(s, c)| s == "world" && c == "begin_transfer")
                .count(),
            2,
            "BOTH members must really be escrowed off the world shard — the live failure was the \
             second one's transfer never running at all: {log:?}"
        );
    });
}

#[test]
fn the_instance_is_mirrored_before_the_character_arrives_in_it() {
    // Ordering, not just presence: `player_login`'s stranding guard DIVERTS a character whose
    // `pending_instance_id` names an instance that does not exist on this shard — so an import
    // that landed before the mirror would put the player outside the dungeon they walked into.
    let (src, dst, _, _, calls) = xdb_pair(None);
    super::transfer::settle_transfer(src.as_ref(), dst.as_ref(), XGUID).unwrap();
    let log = calls.lock().unwrap().clone();
    let mirror = log
        .iter()
        .position(|(_, c)| c == "ensure_instance")
        .expect("mirrored");
    let import = log
        .iter()
        .position(|(_, c)| c == "import_character_blob")
        .expect("imported");
    assert!(
        mirror < import,
        "the instance must exist before the character lands in it: {log:?}"
    );
}

#[test]
fn an_open_world_destination_mirrors_and_evicts_nothing() {
    // Zoning OUT: instance 0 is the open world, which is not an instance and must never be
    // "mirrored" (the module refuses id 0) or evicted (that would tear down the open world).
    let calls: ShardCallLog = Default::default();
    let src_db = FakeShardDb::with_character(
        XGUID,
        FakeChar {
            map_id: 0,
            instance_id: 0,
            payload: "gear+spells".into(),
        },
    );
    let dst_db = FakeShardDb::empty();
    let src = xstore("instances", src_db, calls.clone(), None);
    let dst = xstore("world", dst_db.clone(), calls.clone(), None);
    super::transfer::settle_transfer(src.as_ref(), dst.as_ref(), XGUID).unwrap();

    assert!(dst_db.live(XGUID), "the character must come back out whole");
    let log = calls.lock().unwrap().clone();
    assert!(
        !log.iter()
            .any(|(_, c)| c == "ensure_instance" || c == "evict_instance_population"),
        "instance 0 is the open world — it is neither mirrored nor evicted: {log:?}"
    );
}

#[test]
fn a_character_already_on_its_home_shard_is_not_transferred_but_is_unfenced() {
    // The steady state (every login that does not cross a boundary), PLUS the one crash window
    // that leaves an arrival fence behind with no escrow anywhere to re-drive from: killed between
    // `finish_transfer` and `release_transfer`. Without the speculative release the character would
    // be fenced out of its own login forever.
    let calls: ShardCallLog = Default::default();
    let db = FakeShardDb::with_character(
        XGUID,
        FakeChar {
            map_id: 36,
            instance_id: 7,
            payload: "gear+spells".into(),
        },
    );
    lk(&db.in_rows).insert(XGUID, XGUID); // the orphaned arrival fence
    let home = xstore("instances", db.clone(), calls.clone(), None);
    assert!(!db.live(XGUID), "precondition: the character is fenced");

    super::transfer::settle_transfer(home.as_ref(), home.as_ref(), XGUID).unwrap();
    assert!(db.live(XGUID), "the stranded arrival fence must be cleared");
    let log = calls.lock().unwrap().clone();
    assert_eq!(
        log.iter().filter(|(_, c)| c == "begin_transfer").count(),
        0,
        "a character already on its home shard must never be re-escrowed: {log:?}"
    );
}

#[test]
fn a_second_transfer_of_the_same_character_is_never_swallowed_as_a_replay() {
    // THE REPEAT-TRANSFER CASE (review of #19). The transfer id IS the character guid, so every
    // hop a character ever makes reuses ONE id — and `plan_begin` reads "an out-row OR an in-row
    // filed under this id names this character" as `BeginPlan::Replay`, i.e. `Ok(())`.
    //
    // Reachable state: the character hopped world -> instances and the driver died between
    // `finish_transfer` and `release_transfer`, so the instances shard holds the character AND an
    // unreleased arrival in-row under id == guid. Now it has to hop OUT again (its location is
    // owned by another shard: a shard-map edit, a region reassignment (#23), or a diverted
    // instance re-entry). Without the fence being cleared first, `begin_transfer` on the instances
    // shard replays into `Ok(())` while escrowing NOTHING — and the character is stuck on a shard
    // it can never leave, failing its own login on every attempt, with no operator recourse.
    let calls: ShardCallLog = Default::default();
    let src_db = FakeShardDb::with_character(
        XGUID,
        FakeChar {
            map_id: 0,
            instance_id: 0,
            payload: "gear+spells".into(),
        },
    );
    lk(&src_db.in_rows).insert(XGUID, XGUID); // the previous hop's unreleased fence
    let dst_db = FakeShardDb::empty();
    let src = xstore("instances", src_db.clone(), calls.clone(), None);
    let dst = xstore("world", dst_db.clone(), calls.clone(), None);

    super::transfer::settle_transfer(src.as_ref(), dst.as_ref(), XGUID)
        .expect("a repeat transfer of the same character must actually run");

    assert!(
        !src_db.has(XGUID),
        "the source copy must be destroyed — not silently left behind"
    );
    assert!(
        dst_db.live(XGUID),
        "the character must arrive LIVE on the far side of hop two"
    );
    assert_eq!(
        dst_db.get(XGUID).unwrap().payload,
        "gear+spells",
        "hop two must carry the rows exactly as hop one did"
    );
    assert!(
        src_db.settled() && dst_db.settled(),
        "no escrow row may outlive the second transfer"
    );
    let log = calls.lock().unwrap().clone();
    assert_eq!(
        log.iter()
            .filter(|(s, c)| s == "instances" && c == "begin_transfer")
            .count(),
        1,
        "begin_transfer must have really escrowed, not replayed into a no-op: {log:?}"
    );
}

#[test]
fn a_failed_transfer_fails_the_login_instead_of_entering_the_world_anyway() {
    // A half-moved character must never be let into the world on whichever shard happened to
    // answer. Both outcomes are recoverable (the escrow holds and the next login re-drives it), but
    // only refusing is honest — and entering anyway is how a character ends up live on the shard
    // that is about to have its copy destroyed.
    let (store, _) = sharded_stores();
    let failing = std::sync::Arc::new(InMemoryStore {
        shard: "world".into(),
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        characters: store.characters.clone(),
        login_entity: Some(warrior_entity()),
        settle_error: Some("instances shard unreachable".into()),
        ..Default::default()
    });
    let (mut client, server_end) = UnixStream::pair().unwrap();
    let server = std::thread::spawn(move || run_world_session(server_end, failing.as_ref()));
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    // No login sequence arrives; the session ends with the transfer's error.
    let _ = ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec);
    drop(client);
    let outcome = server.join().unwrap();
    let err = outcome.expect_err("a failed transfer must fail the session, not enter the world");
    assert!(
        format!("{err:#}").contains("instances shard unreachable"),
        "{err:#}"
    );
}

#[test]
fn entering_the_world_binds_this_accounts_identity_on_the_shard_it_landed_on() {
    // A character that arrived via `import_character_blob` has only a SHADOW account row on the
    // destination, with no identity bound — and `world::player_login` resolves its caller through
    // `account_by_identity`. Without this bind the arriving player cannot log in at all, on a
    // database the logon tier never touched.
    let (store, calls) = sharded_stores();
    let home = store
        .home
        .clone()
        .expect("the fixture routes to a home shard");
    let _ = drive_routed_session(store, calls.clone());
    assert_eq!(
        *home.bound_sessions.lock().unwrap(),
        vec![7],
        "the home shard must have this account's identity bound before player_login runs"
    );
    let log = calls.lock().unwrap().clone();
    let bind = log
        .iter()
        .position(|(s, c)| s == "instances" && c == "bind_shard_session");
    let login = log
        .iter()
        .position(|(s, c)| s == "instances" && c == "player_login");
    assert!(
        bind < login && bind.is_some(),
        "the identity must be bound BEFORE player_login, not after: {log:?}"
    );
}

/// ENFORCEMENT tripwire, the module's `body_of` pattern: the production routing read lives on
/// `Coordinator` and needs a live SDK cache, so no mock can drive it — and a mutation of it
/// survived the first cut of this file's mutation pass. Source-scan it instead.
#[test]
fn the_routing_read_uses_the_pending_instance_id_not_a_hardcoded_zero() {
    let src = include_str!("../stdb/reads.rs");
    let start = src
        .find("pub fn character_location(")
        .expect("`character_location` moved — re-derive this tripwire");
    let body = &src[start..start + src[start..].find("\n    }").expect("fn has a body")];
    let code: String = body
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        code.contains("c.pending_instance_id"),
        "character_location no longer reads `pending_instance_id` for a character with no live \
         entity. That column is where `teleport_player` parks the DESTINATION instance for a \
         cross-map hop, so it is the whole routing key for instance entry (#19): reading 0 there \
         routes a player walking into Deadmines by MAP alone, which is correct only until a shard \
         map names a bucket (`389:0=pool-a`, see `config::ShardMap`). Body was:\n{code}"
    );
}

/// Sibling tripwire (same reason — a live SDK cache no mock reaches): a character parked inside a
/// dungeon lives on the INSTANCE shard, and Phase A has no realm-core index to ask.
#[test]
fn the_character_select_list_still_unions_across_every_shard() {
    let ws = include_str!("../stdb/world_store.rs");
    let at = ws
        .find("fn characters(&self, account_id: u64)")
        .expect("`characters` moved");
    assert!(
        ws[at..at + 900].contains("self.all_shards()"),
        "the character-select list no longer unions across shards (#19) — asking only the realm \
         database makes a character that logged out inside an instance vanish from character \
         select entirely, because its durable row is on the instance shard."
    );
}

// The escrow-priority tripwire that used to live here (`locate_character_still_prefers_the_shard_
// holding_the_escrow`, a source scan of `Coordinator::locate_character`) was retired by issue #47:
// `settle_home_shard`'s holder lookup is now `realm_core::locate_home_shard`, generic over the
// `RealmDb` seam, and the escrow-priority property is pinned BEHAVIOURALLY there instead —
// `locate_home_shard_still_prefers_the_shard_holding_the_escrow_in_the_fallback_scan` in
// `realm_core.rs`, which runs the real fallback-scan code against `fake::Handle` rather than
// matching its source text.

/// Sibling tripwire, for #21: what `Coordinator::instance_shard_for` actually FORWARDS.
///
/// `ShardMap::instance_owner` is pinned by its own unit tests and the call site in
/// `settle_home_shard` is pinned by `routing_call_site_tests`, but the three-line adapter between
/// them is reachable from neither — it needs a live `ShardSet`. Verified by mutation: each of the
/// three substitutions below left all 391 gateway tests green while deleting or inverting the
/// stickiness rule outright.
#[test]
fn instance_shard_for_still_forwards_the_holder_the_instance_and_the_connected_set() {
    let conn = include_str!("../stdb/connection.rs");
    let at = conn
        .find("pub(crate) fn instance_shard_for(")
        .expect("`instance_shard_for` moved");
    let body: String = conn[at..at + 500]
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    // (a) the HOLDER, not `self`. `self` is the session's handle — on a login that is the default
    //     shard, which is never a member of a dungeon map's pool, so stickiness could never fire.
    // (b) the real `instance_id`. A literal `0` makes `instance_owner`'s open-world guard reject
    //     every call, and the map decides again — i.e. live runs fork on a pool resize.
    // (c) the CONNECTED predicate. `|_| true` would return a holder the gateway never reached,
    //     which `shard_handle` then cannot resolve, pinning the session to whatever asked.
    assert!(
        body.contains("instance_owner(map_id, instance_id, holder,"),
        "instance_shard_for no longer forwards (map_id, instance_id, holder) verbatim to \
         `ShardMap::instance_owner` (#21). The holder is the ONLY durable evidence of which pool \
         member a live dungeon run is on; substituting `self.shard_name()` or a literal instance \
         id silently restores pre-#21 routing and forks every live run when the operator adds a \
         second instances database. Body was:\n{body}"
    );
    assert!(
        body.contains("self.1.conns.contains_key(d)"),
        "instance_shard_for's `connected` predicate no longer reads the live connection set — a \
         stickiness answer naming a database the gateway never reached cannot be routed to, and \
         must degrade to the shard map like every other resolver in `config.rs`. Body was:\n{body}"
    );
}

#[test]
fn a_resumed_transfer_reuses_the_escrowed_destination_not_the_character_row() {
    // Resume authority: once an escrow exists, ITS destination is the one the destination shard may
    // already hold an imported copy for. Re-deriving from the (frozen) character row instead would
    // drive the second half of the transfer at a different place than the first half imported into.
    let calls: ShardCallLog = Default::default();
    let src_db = FakeShardDb::with_character(
        XGUID,
        FakeChar {
            map_id: 36,
            instance_id: 7,
            payload: "gear+spells".into(),
        },
    );
    lk(&src_db.out_rows).insert(
        XGUID,
        FakeEscrow {
            transfer_id: XGUID,
            character_guid: XGUID,
            dest_map_id: 36,
            dest_instance_id: 42, // ← the escrow's destination, deliberately NOT the row's
            blob: fake_blob(XGUID, 36, 42, "gear+spells"),
        },
    );
    let dst_db = FakeShardDb::empty();
    let src = xstore("world", src_db.clone(), calls.clone(), None);
    let dst = xstore("instances", dst_db.clone(), calls.clone(), None);
    super::transfer::settle_transfer(src.as_ref(), dst.as_ref(), XGUID).unwrap();
    assert_eq!(
        dst_db.get(XGUID).unwrap().instance_id,
        42,
        "the resumed transfer must land where the ESCROW says, not where the character row says"
    );
    assert!(
        lk(&dst_db.instances).contains(&42),
        "and it must mirror the escrow's instance"
    );
}

// ===========================================================================================
//  Issue #209: "SMSG_COMPRESSED_MOVES corrupts at crowd scale" — regression pins.
//
//  Root-cause finding: the gateway never constructs `SMSG_COMPRESSED_MOVES` — a full-repo grep for
//  `compressed_moves`/`COMPRESSED_MOVES`/`flate2` and a `git log -S "COMPRESSED_MOVES" --all` both
//  come up empty. A direct probe of `wow_world_messages` 0.3's own codec for that opcode (encode via
//  `write_into_vec`, decode via `ServerOpcodeMessage::read_unencrypted`) round-trips cleanly through
//  at least 3000 synthetic movers; it only silently truncates its u16 size field past ~9000 movers,
//  an order of magnitude past anything #209 reports. So "compressed moves corrupts" is not an encode
//  defect in that opcode.
//
//  The likelier mechanism: `SMSG_COMPRESSED_MOVES` and `SMSG_COMPRESSED_UPDATE_OBJECT` are the ONLY
//  two decoders in the entire vanilla message set with an internal `.unwrap()` (on a flate2
//  `read_to_end`) instead of a graceful `Result` — every other decoder in `wow_world_messages` 0.3
//  returns `Err` on garbage input. `wow_world_messages`' own framing reads exactly the header's
//  declared `size` bytes into an in-memory buffer BEFORE dispatching to any per-opcode decoder, so a
//  decode-internal panic on THIS opcode cannot itself desync the socket — but if an EARLIER frame's
//  declared size was ever wrong for any reason, every later header decrypts from the wrong stream
//  offset, and the first opcode whose decoder panics instead of erroring is deterministically where
//  the crash surfaces — regardless of what actually caused the desync. That makes this decoder the
//  "weakest link" any unrelated corruption converges on, not necessarily the origin.
//
//  What follows pins the two real, high-volume movement paths — `SMSG_MONSTER_MOVE` typed sends and
//  the `Outbound::Raw` peer-motion relay (work-item 286) — through the ACTUAL `spawn_writer` (the
//  single writer thread that owns the header cipher) with a REAL `wow_srp` cipher pair, at the exact
//  scale #209 reports (100 known-good, 150/300/500 the reported-bad range). If the writer ever
//  mis-declared one frame's size, this is where it would show up: as a decode failure on THIS test,
//  not as an unrelated panic on an opcode the gateway doesn't send.
// ===========================================================================================

/// Push `n` movers' worth of both hot movement paths through a real `spawn_writer` + real cipher
/// pair, then decode every frame back and demand: no decode error, and exactly `n` of each opcode,
/// in order. A stream desync from a wrong size field would manifest here as either a decode `Err`
/// or a frame decoding to the wrong `ServerOpcodeMessage` variant.
fn movement_burst_over_a_real_cipher(n: usize) {
    let store = std::sync::Arc::new(InMemoryStore {
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 42,
            session_key: K,
        }),
        ..Default::default()
    });
    let (mut client, server_end) = UnixStream::pair().unwrap();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        let mut s = server_end;
        let (_conn, encrypt) = world_handshake(&mut s, server_store.as_ref())
            .unwrap()
            .expect("handshake should succeed");
        (s, encrypt)
    });
    let (_c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    let (s, encrypt) = server
        .join()
        .expect("server handshake thread must not panic");

    let (tx, rx, depth) = session_channel();
    let writer = spawn_writer(s, encrypt, rx, depth, 1).expect("spawn the writer thread");

    // The reader MUST run concurrently with the writer, not after `writer.join()` — a Unix domain
    // socket's kernel send buffer is finite, and a burst past it makes the writer's `write_all`
    // block waiting for a reader that (if it only starts after the writer finishes) never comes:
    // a real deadlock, caught empirically writing this test (it hung indefinitely at n=500 with
    // both threads idle, not merely slow). Draining while the writer produces mirrors what a real
    // socket pair does in production — neither side ever waits on a queue nobody is draining.
    let reader = std::thread::spawn(move || {
        let mut monster_moves = 0usize;
        let mut heartbeats = 0usize;
        for _ in 0..(2 * n) {
            match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec) {
                Ok(ServerOpcodeMessage::SMSG_MONSTER_MOVE(_)) => monster_moves += 1,
                Ok(ServerOpcodeMessage::MSG_MOVE_HEARTBEAT(_)) => heartbeats += 1,
                Ok(other) => panic!(
                    "n={n}: decoded an unexpected opcode after {monster_moves} monster-moves + \
                     {heartbeats} heartbeats — this IS a stream desync: {other}"
                ),
                Err(e) => panic!(
                    "n={n}: decode failed after {monster_moves} monster-moves + {heartbeats} \
                     heartbeats — exactly the shape of #209's crash (a desynced header decoding to \
                     garbage): {e}"
                ),
            }
        }
        (monster_moves, heartbeats)
    });

    for i in 0..n {
        let guid = 0x1000_0000_0000_0000u64 + i as u64;
        let start = wow_world_messages::vanilla::Vector3d {
            x: -8938.0 + i as f32,
            y: -131.0,
            z: 83.5,
        };
        let dest = wow_world_messages::vanilla::Vector3d {
            x: -8900.0 + i as f32,
            y: -100.0,
            z: 82.4,
        };
        let m = codec::build_monster_move(guid, start, dest, 500, i as u32 + 1, i % 2 == 0);
        tx.send(Outbound::One(ServerOpcodeMessage::SMSG_MONSTER_MOVE(
            Box::new(m),
        )))
        .expect("writer thread must still be alive mid-burst");

        let info = MovementInfo {
            flags: wow_world_messages::vanilla::MovementInfo_MovementFlags::empty(),
            timestamp: i as u32,
            position: wow_world_messages::vanilla::Vector3d {
                x: 100.0 + i as f32,
                y: 0.0,
                z: 0.0,
            },
            orientation: 0.0,
            fall_time: 0.0,
        };
        let info_bytes = codec::movement_info_to_bytes(&info)
            .expect("a well-formed MovementInfo must serialize");
        let (opcode, body) = codec::build_movement_relay_raw(
            lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT,
            guid,
            &info_bytes,
        )
        .expect("a valid MovementInfo carrier must always produce a relay frame");
        tx.send(Outbound::Raw { opcode, body })
            .expect("writer thread must still be alive mid-burst");
    }
    drop(tx);
    writer
        .join()
        .expect("writer thread must not panic under a movement burst");
    let (monster_moves, heartbeats) = reader.join().expect("reader thread must not panic");

    assert_eq!(
        monster_moves, n,
        "n={n}: lost or gained a typed SMSG_MONSTER_MOVE frame"
    );
    assert_eq!(
        heartbeats, n,
        "n={n}: lost or gained a raw peer-motion relay frame"
    );
}

#[test]
fn writer_thread_survives_movement_bursts_from_100_to_500_movers_without_desync() {
    // 100 is the report's own known-good control; 150/300/500 span and exceed the reported-bad
    // range (150+) and the natural live-validation target (a 300-mage raid, #209's own repro line).
    for n in [100usize, 150, 300, 500] {
        movement_burst_over_a_real_cipher(n);
    }
}

/// #209 probe (writer-side black box): [`WriterTrace::record`] must recover the SAME `(opcode,
/// size)` pair for the typed `Outbound::One` path as the wire actually carries — pinned against a
/// real gtker message rather than trusting the `write_unencrypted_server` re-derivation blind.
/// `SMSG_MONSTER_MOVE`'s body length is data-dependent (spline point count), so this also proves
/// the ring doesn't hard-code a size.
#[test]
fn writer_trace_records_the_typed_path_opcode_and_size() {
    let m = codec::build_monster_move(
        0xF130_0000_0000_0001,
        wow_world_messages::vanilla::Vector3d {
            x: -8938.0,
            y: -131.0,
            z: 83.5,
        },
        wow_world_messages::vanilla::Vector3d {
            x: -8900.0,
            y: -100.0,
            z: 82.4,
        },
        500,
        1,
        true,
    );
    let msg = ServerOpcodeMessage::SMSG_MONSTER_MOVE(Box::new(m));
    let mut plain = Vec::new();
    msg.write_unencrypted_server(&mut plain).unwrap();
    let expected_size = u16::from_be_bytes([plain[0], plain[1]]);
    let expected_opcode = u16::from_le_bytes([plain[2], plain[3]]);
    let expected_checksum = fnv1a64(&plain[4..]);

    let mut trace = WriterTrace::new();
    trace.record(&Outbound::One(msg));

    assert_eq!(
        trace.ring.len(),
        1,
        "one Outbound::One must record exactly one frame"
    );
    let entry = trace.ring[0];
    assert_eq!(entry.opcode, expected_opcode);
    assert_eq!(entry.size, expected_size);
    assert_eq!(entry.checksum, expected_checksum);
}

/// A `Batch` of N messages is N frames on the wire (the writer sends each contiguously) — the ring
/// must expand it to N entries, not collapse it to one, or a diff against the harness's per-frame
/// dump would silently misalign after the first batch.
#[test]
fn writer_trace_expands_a_batch_into_one_entry_per_message() {
    let make = |i: u32| {
        let m = codec::build_monster_move(
            0x2000_0000_0000_0000 + i as u64,
            wow_world_messages::vanilla::Vector3d {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            wow_world_messages::vanilla::Vector3d {
                x: 1.0,
                y: 1.0,
                z: 1.0,
            },
            100,
            i + 1,
            false,
        );
        ServerOpcodeMessage::SMSG_MONSTER_MOVE(Box::new(m))
    };
    let mut trace = WriterTrace::new();
    trace.record(&Outbound::Batch(vec![make(0), make(1), make(2)]));
    assert_eq!(
        trace.ring.len(),
        3,
        "a 3-message Batch must expand to 3 traced frames"
    );
}

/// `Outbound::Raw`'s entry must reflect the EXACT body handed to it — `size` is `2 + body.len()`
/// (matching `spawn_writer`'s own framing, not gtker's), and the checksum is over the raw body
/// bytes with no re-serialization in between (Raw never touches gtker's codec).
#[test]
fn writer_trace_records_the_raw_path_verbatim() {
    let body = vec![0xAAu8, 0xBB, 0xCC, 0xDD, 0xEE];
    let mut trace = WriterTrace::new();
    trace.record(&Outbound::Raw {
        opcode: 0x00EE,
        body: body.clone(),
    });
    let entry = trace.ring[0];
    assert_eq!(entry.opcode, 0x00EE);
    assert_eq!(entry.size, 2 + body.len() as u16);
    assert_eq!(entry.checksum, fnv1a64(&body));
}

/// The ring is bounded at [`WriterTrace::CAPACITY`] — a long-lived session must not grow this
/// unboundedly; pushing past capacity drops the OLDEST entry (a FIFO ring, oldest-first on dump).
#[test]
fn writer_trace_ring_caps_at_capacity_and_drops_oldest() {
    let mut trace = WriterTrace::new();
    for i in 0..(WriterTrace::CAPACITY + 5) {
        trace.push(i as u16, 0, 0);
    }
    assert_eq!(trace.ring.len(), WriterTrace::CAPACITY);
    // The oldest 5 pushes (opcodes 0..5) must have been evicted; the ring now starts at 5.
    assert_eq!(trace.ring.front().unwrap().opcode, 5);
    assert_eq!(
        trace.ring.back().unwrap().opcode,
        (WriterTrace::CAPACITY + 4) as u16
    );
}

/// `dump` writes a real file under `/tmp/gw-writer-crash/<account_id>.txt` — the whole point of the
/// black box is that it survives the process exiting the way #209's crash does. Uses a distinctive
/// account id to avoid colliding with a real session's dump from a live repro run on the same box.
#[test]
fn writer_trace_dump_writes_a_file_with_the_traced_frames() {
    let mut trace = WriterTrace::new();
    trace.push(0x00EE, 31, 0xDEAD_BEEF_0000_0001);
    trace.push(0x0130, 6, 0xDEAD_BEEF_0000_0002);
    const ACCOUNT: u64 = 0x0FFF_FFFF_F209_0000; // won't collide with a real bench account id
    trace.dump(ACCOUNT, "test-induced dump, not a real session end");

    let path = format!("/tmp/gw-writer-crash/{ACCOUNT}.txt");
    let contents = std::fs::read_to_string(&path).expect("dump must write a readable file");
    assert!(
        contents.contains("opcode=0x00EE"),
        "missing first frame: {contents}"
    );
    assert!(
        contents.contains("opcode=0x0130"),
        "missing second frame: {contents}"
    );
    assert!(
        contents.contains("test-induced dump"),
        "missing the end reason: {contents}"
    );
    let _ = std::fs::remove_file(&path); // leave /tmp clean for a real repro's dumps
}

/// #209 hardening (this diff's actual fix): an `Outbound::Raw` body that overflows the u16 frame-size
/// field must end the session instead of silently wrapping the declared size — the wrap is exactly
/// the "one wrong size field desyncs every later header" mechanism this whole investigation chased.
/// No current builder produces a body this large (confirmed by reading every `Outbound::Raw` call
/// site), so this exercises the guard directly rather than waiting for a caller to reach it.
///
/// Mutation-checked by hand: reverting this arm to the old `debug_assert!`-only form makes this test
/// FAIL — in `cargo test`'s own debug profile, the reinstated `debug_assert!` fires and panics the
/// `world-writer` thread (caught here as `writer.join()` returning `Err`, not `Ok`). That panic is a
/// debug-profile-only tripwire, though: in the RELEASE profile the capacity benchmark and any live
/// deploy actually run, `debug_assert!` compiles out entirely and the `as u16` cast just wraps
/// silently — a header claiming a tiny size, followed by the full oversized body, corrupting every
/// packet after it. The mutation is caught here either way (panic in debug, corruption in release);
/// this test's own build only exercises the debug-panic half of that story.
#[test]
fn oversized_raw_body_ends_the_session_instead_of_wrapping_the_size_field() {
    let store = std::sync::Arc::new(InMemoryStore {
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 42,
            session_key: K,
        }),
        ..Default::default()
    });
    let (mut client, server_end) = UnixStream::pair().unwrap();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        let mut s = server_end;
        let (_conn, encrypt) = world_handshake(&mut s, server_store.as_ref())
            .unwrap()
            .expect("handshake should succeed");
        (s, encrypt)
    });
    let (_c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    let (s, encrypt) = server
        .join()
        .expect("server handshake thread must not panic");

    let (tx, rx, depth) = session_channel();
    let writer = spawn_writer(s, encrypt, rx, depth, 1).expect("spawn the writer thread");

    // 2 (opcode) + this body must exceed u16::MAX to hit the guard.
    let oversized_body = vec![0xABu8; u16::MAX as usize];
    tx.send(Outbound::Raw {
        opcode: 0x00A9,
        body: oversized_body,
    })
    .unwrap();
    drop(tx);

    // The guard must make the writer end the connection cleanly, not hang and not panic.
    writer
        .join()
        .expect("the writer thread must not panic on an oversized raw body");

    // Nothing valid was ever written for this frame: the client sees EOF, not a bogus small-size
    // frame followed by the oversized body. `read_encrypted` reads a 4-byte header first, so with
    // zero bytes on the wire this must fail (EOF), never `Ok`.
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec) {
        Err(_) => {}
        Ok(m) => panic!(
            "expected a clean EOF (nothing sent for the oversized frame), but decoded {m} — the \
             size field wrapped and the client just parsed a corrupt frame as if it were real"
        ),
    }
}

// ===========================================================================================
//  Issue #209, discriminator round 2 (2026-08-04): movement-only load stayed clean at 500 co-located
//  movers (the test above), but the real repro — a mage-boss raid pull, `--cast-spell 133` — crashes
//  at 150 co-located CASTERS. That narrows the corrupt frame to the SPELL/COMBAT relay plane, not
//  movement. This mirrors `movement_burst_over_a_real_cipher` above (PR #218's rig) for the frames
//  that plane actually emits per cast, enumerated by reading every `Outbound::One`/`Outbound::Raw`
//  send site on the cast/combat path (`gateway/src/stdb/subscriptions.rs`'s `on_cast`/`on_combat`
//  listeners, and `world/mod.rs`'s synchronous `CMSG_CAST_SPELL` handler):
//
//    SMSG_SPELL_START            - cast-begin (subscriptions.rs on_cast; world/mod.rs sync ack)
//    SMSG_SPELL_GO               - cast visual/finalize (subscriptions.rs on_cast + on_combat ranged;
//                                   world/mod.rs sync ack)
//    SMSG_SPELLNONMELEEDAMAGELOG - floating spell/ranged damage number (subscriptions.rs on_cast,
//                                   on_combat ranged, on_impact projectile-land)
//    SMSG_SPELL_COOLDOWN         - cooldown swipe, only for spells that have one (subscriptions.rs
//                                   on_cast)
//    SMSG_ATTACKERSTATEUPDATE    - melee swing (subscriptions.rs on_combat, non-ranged non-spell-swing)
//    SMSG_SPELL_FAILURE/SMSG_SPELL_DELAYED - interrupt/pushback signals (subscriptions.rs on_cast) —
//                                   rarer than the per-cast steady state above, not swept here
//    Outbound::Raw{0x0130} SMSG_CAST_RESULT - the caster-only cast ACK (subscriptions.rs on_cast,
//                                   world/mod.rs sync handler). DELIBERATELY EXCLUDED from this rig's
//                                   round-trip below: gtker's own typed `SMSG_CAST_RESULT` decoder has
//                                   the Success/Failure branch condition INVERTED from the real 1.12
//                                   protocol (see `codec::combat::build_cast_result_ok`'s doc comment —
//                                   this is why the gateway sends it via `Outbound::Raw` at all). A
//                                   5-byte OK body (spell_id + 0x00) makes gtker's decoder try to read
//                                   a Success-only "reason" byte that was never sent, which errors —
//                                   correctly, for the real client, but as an artifact of THIS crate's
//                                   own decoder, not of anything the gateway did wrong. Asserting this
//                                   opcode round-trips through `ServerOpcodeMessage::read_encrypted`
//                                   here would just fail on that pre-existing, unrelated gap and add
//                                   noise to the #209 hunt, not signal.
//
//  Same rig as the movement test: real `spawn_writer`, real `wow_srp` cipher pair, a concurrently
//  draining reader (the same deadlock trap applies), interleaved with the SAME movement heartbeat
//  relay PR #218 already pinned clean — because the real repro is combat+cast ON TOP OF movement,
//  not combat alone.
// ===========================================================================================

/// Push `n` casters' worth of one full per-cast sequence (SPELL_START → SPELL_GO →
/// SPELLNONMELEEDAMAGELOG → SPELL_COOLDOWN, mirroring a single boss-target Fireball rotation) PLUS one
/// boss-swing ATTACKERSTATEUPDATE and one peer-motion heartbeat, through a real `spawn_writer` + real
/// cipher pair, then decode every frame back and demand: no decode error, and exactly `n` of each
/// opcode, in order. A stream desync from a wrong size field on ANY of these opcodes would manifest
/// here as a decode `Err` or a wrong-variant decode — precisely #209's crash shape, but attributable
/// to the SPELL/COMBAT plane instead of movement.
fn combat_cast_burst_over_a_real_cipher(n: usize) {
    let store = std::sync::Arc::new(InMemoryStore {
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 42,
            session_key: K,
        }),
        ..Default::default()
    });
    let (mut client, server_end) = UnixStream::pair().unwrap();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        let mut s = server_end;
        let (_conn, encrypt) = world_handshake(&mut s, server_store.as_ref())
            .unwrap()
            .expect("handshake should succeed");
        (s, encrypt)
    });
    let (_c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    let (s, encrypt) = server
        .join()
        .expect("server handshake thread must not panic");

    let (tx, rx, depth) = session_channel();
    let writer = spawn_writer(s, encrypt, rx, depth, 1).expect("spawn the writer thread");

    const SPELL_ID: u32 = 133; // Fireball — the #209 repro's own `--cast-spell 133`
    const BOSS_GUID: u64 = 0xF130_0000_0000_0001; // one shared raid target, mirrors raid-mode BENCH_ARGS
    const FRAMES_PER_CASTER: usize = 6; // START + GO + DAMAGE_LOG + COOLDOWN + ATTACKERSTATEUPDATE + heartbeat

    // The reader MUST run concurrently with the writer — see `movement_burst_over_a_real_cipher`'s
    // comment on the exact same deadlock this rig hit empirically at n=500 there.
    let reader = std::thread::spawn(move || {
        let mut starts = 0usize;
        let mut goes = 0usize;
        let mut damage_logs = 0usize;
        let mut cooldowns = 0usize;
        let mut swings = 0usize;
        let mut heartbeats = 0usize;
        for _ in 0..(FRAMES_PER_CASTER * n) {
            match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec) {
                Ok(ServerOpcodeMessage::SMSG_SPELL_START(_)) => starts += 1,
                Ok(ServerOpcodeMessage::SMSG_SPELL_GO(_)) => goes += 1,
                Ok(ServerOpcodeMessage::SMSG_SPELLNONMELEEDAMAGELOG(_)) => damage_logs += 1,
                Ok(ServerOpcodeMessage::SMSG_SPELL_COOLDOWN(_)) => cooldowns += 1,
                Ok(ServerOpcodeMessage::SMSG_ATTACKERSTATEUPDATE(_)) => swings += 1,
                Ok(ServerOpcodeMessage::MSG_MOVE_HEARTBEAT(_)) => heartbeats += 1,
                Ok(other) => panic!(
                    "n={n}: decoded an unexpected opcode after {starts} starts + {goes} goes + \
                     {damage_logs} damage-logs + {cooldowns} cooldowns + {swings} swings + \
                     {heartbeats} heartbeats — this IS a stream desync, on the SPELL/COMBAT plane: \
                     {other}"
                ),
                Err(e) => panic!(
                    "n={n}: decode failed after {starts} starts + {goes} goes + {damage_logs} \
                     damage-logs + {cooldowns} cooldowns + {swings} swings + {heartbeats} \
                     heartbeats — exactly #209's crash shape, on the SPELL/COMBAT plane: {e}"
                ),
            }
        }
        (starts, goes, damage_logs, cooldowns, swings, heartbeats)
    });

    for i in 0..n {
        let caster = 0x1000_0000_0000_0000u64 + i as u64;
        let damage = 40 + (i as u32 % 60); // varies the body, like a real crit/resist spread would

        let start = codec::build_spell_start(caster, SPELL_ID, 0, 0, None);
        tx.send(Outbound::One(ServerOpcodeMessage::SMSG_SPELL_START(
            Box::new(start),
        )))
        .expect("writer thread must still be alive mid-burst");

        let go = codec::build_spell_go(caster, SPELL_ID, BOSS_GUID, None);
        tx.send(Outbound::One(ServerOpcodeMessage::SMSG_SPELL_GO(Box::new(
            go,
        ))))
        .expect("writer thread must still be alive mid-burst");

        let log = codec::build_spell_non_melee_damage_log(
            BOSS_GUID,
            caster,
            SPELL_ID,
            damage,
            2, /* fire */
            i % 5 == 0,
            0,
            0,
        );
        tx.send(Outbound::One(
            ServerOpcodeMessage::SMSG_SPELLNONMELEEDAMAGELOG(Box::new(log)),
        ))
        .expect("writer thread must still be alive mid-burst");

        let cd = codec::build_spell_cooldown(caster, SPELL_ID, 1500);
        tx.send(Outbound::One(ServerOpcodeMessage::SMSG_SPELL_COOLDOWN(
            Box::new(cd),
        )))
        .expect("writer thread must still be alive mid-burst");

        // The boss swinging back at whichever caster it's leashed onto this tick — real raid load is
        // casts AND incoming melee at once, not casts in isolation.
        let swing = codec::build_attacker_state_update(BOSS_GUID, caster, damage, 0, 0, 0);
        tx.send(Outbound::One(
            ServerOpcodeMessage::SMSG_ATTACKERSTATEUPDATE(Box::new(swing)),
        ))
        .expect("writer thread must still be alive mid-burst");

        let info = MovementInfo {
            flags: wow_world_messages::vanilla::MovementInfo_MovementFlags::empty(),
            timestamp: i as u32,
            position: wow_world_messages::vanilla::Vector3d {
                x: -8938.0 + i as f32,
                y: -131.0,
                z: 83.5,
            },
            orientation: 0.0,
            fall_time: 0.0,
        };
        let info_bytes = codec::movement_info_to_bytes(&info)
            .expect("a well-formed MovementInfo must serialize");
        let (opcode, body) = codec::build_movement_relay_raw(
            lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT,
            caster,
            &info_bytes,
        )
        .expect("a valid MovementInfo carrier must always produce a relay frame");
        tx.send(Outbound::Raw { opcode, body })
            .expect("writer thread must still be alive mid-burst");
    }
    drop(tx);
    writer
        .join()
        .expect("writer thread must not panic under a combat+cast burst");
    let (starts, goes, damage_logs, cooldowns, swings, heartbeats) =
        reader.join().expect("reader thread must not panic");

    assert_eq!(starts, n, "n={n}: lost or gained a SMSG_SPELL_START frame");
    assert_eq!(goes, n, "n={n}: lost or gained a SMSG_SPELL_GO frame");
    assert_eq!(
        damage_logs, n,
        "n={n}: lost or gained a SMSG_SPELLNONMELEEDAMAGELOG frame"
    );
    assert_eq!(
        cooldowns, n,
        "n={n}: lost or gained a SMSG_SPELL_COOLDOWN frame"
    );
    assert_eq!(
        swings, n,
        "n={n}: lost or gained a SMSG_ATTACKERSTATEUPDATE frame"
    );
    assert_eq!(
        heartbeats, n,
        "n={n}: lost or gained a peer-motion relay frame"
    );
}

#[test]
fn writer_thread_survives_combat_cast_bursts_from_100_to_300_casters_without_desync() {
    // 100 = clean per the movement test's own control range; 150 = the discriminator's OWN reported
    // crash threshold ("crashes at 150" combat+cast, vs. clean at 500 movement-only); 200/300 span and
    // exceed it (300 is the natural raid-repro target — `BENCH_ARGS="--class mage --boss <hogger>
    // --cast-spell 133"`, a 300-mage raid). If this ever goes red, the failure message above names
    // the exact opcode and frame-count offset where the SPELL/COMBAT plane desynced — the #209 root
    // cause, headlessly. If it stays green through all four, the corrupt frame is NOT produced by any
    // of these send paths, and the next probe is the byte-level capture (the wire harness's
    // `/tmp/wire-crash/` dump, issue #209 deliverable 1) taken at the moment of a LIVE repro.
    for n in [100usize, 150, 200, 300] {
        combat_cast_burst_over_a_real_cipher(n);
    }
}
