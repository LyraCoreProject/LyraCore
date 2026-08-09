//! Social tier — world chat (Say/Yell) + emotes (`/dance`, `/wave`, …) + party chat (`/p`). A
//! player's `CMSG_MESSAGECHAT` or `CMSG_TEXT_EMOTE` becomes a per-recipient or broadcast event row
//! that the gateway fans to clients as `SMSG_MESSAGECHAT` / `SMSG_TEXT_EMOTE` (+ `SMSG_EMOTE`
//! animation), mirroring the creature-move / combat broadcast relays. Purely additive: new tables +
//! reducers, reaped by the shared event GC. Range filtering (say ~25yd, yell ~300yd) and targeted
//! emotes ("waves at X") are later refinements — today say/yell broadcast like the other event
//! relays (single starting zone). Party chat (work-item 199) is NOT proximity-based — it rides the
//! group system's per-recipient `game_group_event` relay (`crate::group`), not a broadcast. [event]

use spacetimedb::{
    client_visibility_filter, reducer, table, Filter, Identity, ReducerContext, Table, Timestamp,
};

use crate::game_character;
use crate::helpers::entity_by_owner;
use lyracore_shared::group::{err as group_err, event_kind as group_event_kind};

// `game_character_contact` is defined further down in THIS module — its `#[table]` accessor
// (`game_character_contact`) is generated there, so it's in scope for `add_friend`/etc. without a
// `use`. Re-exported implicitly like `game_whisper_event` above.

/// `game_chat_event.chat_type` discriminants for the two BROADCAST chat types this table carries.
/// Whisper and party (work-item 199) are NOT `game_chat_event` rows at all — whisper rides
/// `game_whisper_event`, party rides `game_group_event` (`party_chat` below) — each has its own
/// per-recipient shape that doesn't fit this broadcast table's `chat_type` byte. Guild/channel still
/// need systems that don't exist yet, so they're rejected.
pub const CHAT_SAY: u8 = 0;
pub const CHAT_YELL: u8 = 1;

/// Max stored message length — vanilla caps client input around 255; we hard-cap to bound the row.
const MAX_CHAT_LEN: usize = 255;

/// A spoken line to fan out as `SMSG_MESSAGECHAT`. Public broadcast (no RLS), like `game_combat_event`;
/// the gateway relays each `on_insert`. Reaped by the shared event GC (same `id` + `created_at` shape). [event]
#[table(accessor = game_chat_event, public)]
pub struct ChatEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub sender_guid: u64,
    pub chat_type: u8, // CHAT_SAY / CHAT_YELL
    pub language: u8,  // vanilla Language discriminant, echoed back to clients
    pub message: String,
    pub created_at: Timestamp,
}

/// True for the chat types this slice relays (say/yell). [pure]
pub fn is_supported_chat_type(chat_type: u8) -> bool {
    chat_type == CHAT_SAY || chat_type == CHAT_YELL
}

/// Trim + length-cap a chat line, returning `None` when nothing is left (so an empty/whitespace line
/// is rejected rather than broadcast). Pure — unit-tested. [pure]
pub fn normalized_message(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(MAX_CHAT_LEN).collect())
}

/// Speak (`CMSG_MESSAGECHAT`). Tracer scope: SAY and YELL only, broadcast world-wide (range filtering
/// is a later refinement). Authorized via `ctx.sender()` like the other player ops. Trims + length-caps
/// the message and rejects an empty one; an unsupported chat type is a clean `Err` (the gateway drops it).
#[reducer]
pub fn send_chat(
    ctx: &ReducerContext,
    chat_type: u8,
    language: u8,
    message: String,
) -> Result<(), String> {
    let sender =
        entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "speaker not in world".to_string())?;
    apply_send_chat(ctx, sender, chat_type, language, message)
}

/// The say/yell core, actor-explicit (#468 stage 4a): everything `send_chat` does after resolving
/// WHO spoke — the sender reducer above and `gw::gw_send_chat` both delegate here.
pub(crate) fn apply_send_chat(
    ctx: &ReducerContext,
    sender: crate::WorldEntity,
    chat_type: u8,
    language: u8,
    message: String,
) -> Result<(), String> {
    // Vanilla: a dead/ghost player can't be heard via Say/Yell (proximity chat). Whisper + party/guild
    // are NOT gated by death (and aren't routed here anyway — this reducer only handles SAY/YELL).
    if sender.dead {
        return Err("dead players cannot speak".to_string());
    }
    if !is_supported_chat_type(chat_type) {
        return Err(format!("unsupported chat type {chat_type}"));
    }
    let text = normalized_message(&message).ok_or_else(|| "empty message".to_string())?;
    ctx.db.game_chat_event().insert(ChatEvent {
        id: 0,
        sender_guid: sender.guid,
        chat_type,
        language,
        message: text,
        created_at: ctx.timestamp,
    });
    Ok(())
}

// ===========================================================================================
//  Chat channels (065): General/Trade/LocalDefense — the client auto-joins on zone-in.
// ===========================================================================================

/// One (channel, character) membership row (065). PUBLIC broadcast-shape: every player connection
/// checks its OWN membership when relaying a `ChannelEvent` (small table, self-filter). `channel`
/// is the NORMALIZED key (lowercased trimmed full client string, zone suffix included — "general -
/// elwynn forest"), so casing can't split a room; the display form rides each event row. Rows
/// persist across relogs (the client re-sends JOIN every zone-in; `join_channel` dedupes). [entity]
#[table(accessor = game_channel_member, public, index(accessor = by_channel, btree(columns = [channel])))]
pub struct ChannelMember {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub channel: String,
    pub character_guid: u64,
    pub owner_identity: Identity,
}

/// A channel line to fan out (065) — its own table per the one-table-per-delivery-shape rule (say/
/// yell broadcast vs whisper RLS vs party group-event): channel delivery = "every MEMBER anywhere",
/// which fits neither. Public; reaped by the shared event GC (`id` + `created_at`). [event]
#[table(accessor = game_channel_event, public)]
pub struct ChannelEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub channel: String,         // normalized key (matches ChannelMember.channel)
    pub channel_display: String, // as the sender's client spelled it (the wire echoes this)
    pub sender_guid: u64,
    pub message: String,
    pub created_at: Timestamp,
}

/// Lowercase+trim a client channel string into the membership key. [pure]
pub fn normalize_channel(name: &str) -> String {
    name.trim().to_ascii_lowercase()
}

/// Join `channel` (065, CMSG_JOIN_CHANNEL): dedupe on (key, character). The gateway acks with
/// SMSG_CHANNEL_NOTIFY(YouJoined) unconditionally — vanilla treats a re-join as idempotent.
#[reducer]
pub fn join_channel(ctx: &ReducerContext, channel: String) -> Result<(), String> {
    let sender =
        entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "joiner not in world".to_string())?;
    apply_join_channel(ctx, sender, channel)
}

/// The channel-join core, actor-explicit (#479) — same split as [`apply_send_chat`].
pub(crate) fn apply_join_channel(
    ctx: &ReducerContext,
    sender: crate::WorldEntity,
    channel: String,
) -> Result<(), String> {
    let key = normalize_channel(&channel);
    if key.is_empty() {
        return Err("empty channel name".to_string());
    }
    let members = ctx.db.game_channel_member();
    let present = members
        .by_channel()
        .filter(&key)
        .any(|m| m.character_guid == sender.guid);
    if !present {
        members.insert(ChannelMember {
            id: 0,
            channel: key,
            character_guid: sender.guid,
            // The MEMBER's own binding, not `ctx.sender()` — identical on the sender path (the
            // entity was resolved BY that identity) and correct on the gateway path, where
            // `ctx.sender()` is the shared connection's operator identity.
            owner_identity: sender.owner_identity,
        });
    }
    Ok(())
}

/// Leave `channel` (065, CMSG_LEAVE_CHANNEL). Idempotent — leaving a channel you're not in is a no-op.
#[reducer]
pub fn leave_channel(ctx: &ReducerContext, channel: String) -> Result<(), String> {
    let sender =
        entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "leaver not in world".to_string())?;
    apply_leave_channel(ctx, sender, channel)
}

/// The channel-leave core, actor-explicit (#479) — same split as [`apply_send_chat`].
pub(crate) fn apply_leave_channel(
    ctx: &ReducerContext,
    sender: crate::WorldEntity,
    channel: String,
) -> Result<(), String> {
    let key = normalize_channel(&channel);
    let members = ctx.db.game_channel_member();
    let ids: Vec<u64> = members
        .by_channel()
        .filter(&key)
        .filter(|m| m.character_guid == sender.guid)
        .map(|m| m.id)
        .collect();
    for id in ids {
        members.id().delete(id);
    }
    Ok(())
}

/// Speak into `channel` (065, the CMSG_MESSAGECHAT Channel arm): sender must be a member (the
/// client can't normally send to an un-joined channel — a modified one gets an Err). Same
/// dead-guard + length cap as say/yell.
#[reducer]
pub fn send_channel_message(
    ctx: &ReducerContext,
    channel: String,
    message: String,
) -> Result<(), String> {
    let sender =
        entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "speaker not in world".to_string())?;
    apply_send_channel_message(ctx, sender, channel, message)
}

/// The channel-speak core, actor-explicit (#479) — same split as [`apply_send_chat`].
pub(crate) fn apply_send_channel_message(
    ctx: &ReducerContext,
    sender: crate::WorldEntity,
    channel: String,
    message: String,
) -> Result<(), String> {
    if sender.dead {
        return Err("dead players cannot speak".to_string());
    }
    let key = normalize_channel(&channel);
    let member = ctx
        .db
        .game_channel_member()
        .by_channel()
        .filter(&key)
        .any(|m| m.character_guid == sender.guid);
    if !member {
        return Err(format!("not in channel {channel}"));
    }
    let text = normalized_message(&message).ok_or_else(|| "empty message".to_string())?;
    ctx.db.game_channel_event().insert(ChannelEvent {
        id: 0,
        channel: key,
        channel_display: channel.trim().to_string(),
        sender_guid: sender.guid,
        message: text,
        created_at: ctx.timestamp,
    });
    Ok(())
}

/// A social emote to fan out — the "X dances." chat line (`SMSG_TEXT_EMOTE`) plus the animation
/// (`SMSG_EMOTE`). Public broadcast (no RLS), reaped by the shared event GC (`id` + `created_at`).
/// `emote_anim` is the client-provided `Emote` animation id; an unknown id degrades to text-only.
/// `target_guid` is the CMSG-supplied target (0 = untargeted); the gateway resolves it to a name so
/// the chat line reads "X waves at Y." instead of "X waves." `#[default(0)]` — additive END-append,
/// existing rows (untargeted, pre-migration) read back as 0 = no target. [event]
#[table(
    accessor = game_emote_event,
    public,
    // perf catalog 2.3: AOI-box scoping instead of a global `SELECT *`.
    index(accessor = by_grid, btree(columns = [map_id, instance_id, grid_x, grid_y]))
)]
pub struct EmoteEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub sender_guid: u64,
    pub text_emote: u32, // the TextEmote id (drives the "X dances." chat line)
    pub emote_anim: u32, // the Emote animation id (observers play it; unknown → no animation)
    pub created_at: Timestamp,
    #[default(0u64)]
    pub target_guid: u64, // CMSG_TEXT_EMOTE's target guid; 0 = untargeted
    // --- AOI columns (perf catalog 2.3), END-appended + TYPED defaults (a bare `0` on a u64
    // encodes as 4 bytes and fails the publish). Stamped from the actor via `helpers::grid_of`;
    // (0,0,0,0) means "no live actor", which matches no box and is correctly never delivered.
    #[default(0u32)]
    pub map_id: u32,
    #[default(0u64)]
    pub instance_id: u64,
    #[default(0i32)]
    pub grid_x: i32,
    #[default(0i32)]
    pub grid_y: i32,
}

/// Perform a text emote (`CMSG_TEXT_EMOTE`): broadcast the social line + animation to observers.
/// `target_guid` (0 = untargeted) is the client-supplied target; the gateway resolves it to a name
/// for the "waves at <name>" line. Authorized via `ctx.sender()` like the other player ops. The
/// `text_emote` / `emote_anim` ids come from the client; invalid ones degrade gracefully gateway-side
/// (the text line is skipped / the animation is dropped) rather than erroring.
#[reducer]
pub fn send_emote(
    ctx: &ReducerContext,
    text_emote: u32,
    emote_anim: u32,
    target_guid: u64,
) -> Result<(), String> {
    let sender =
        entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "emoter not in world".to_string())?;
    apply_send_emote(ctx, sender, text_emote, emote_anim, target_guid)
}

/// The text-emote core, actor-explicit (#468 stage 4a) — same split as [`apply_send_chat`].
pub(crate) fn apply_send_emote(
    ctx: &ReducerContext,
    sender: crate::WorldEntity,
    text_emote: u32,
    emote_anim: u32,
    target_guid: u64,
) -> Result<(), String> {
    // Vanilla: a dead/ghost player can't perform a (proximity) social emote.
    if sender.dead {
        return Err("dead players cannot emote".to_string());
    }
    // perf catalog 2.3: AOI address of the actor — `sender` is already the live entity in hand, so
    // this stamps directly off it (`entity_addr`) instead of paying a redundant `grid_of` PK lookup.
    let (map_id, instance_id, grid_x, grid_y) = crate::helpers::entity_addr(&sender);
    ctx.db.game_emote_event().insert(EmoteEvent {
        id: 0,
        sender_guid: sender.guid,
        text_emote,
        emote_anim,
        created_at: ctx.timestamp,
        target_guid,
        map_id,
        instance_id,
        grid_x,
        grid_y,
    });
    Ok(())
}

// ===========================================================================================
//  Whisper [event] — RLS-scoped to one recipient (unlike say/yell/emote, which broadcast)
// ===========================================================================================

/// A whisper line delivered to ONE recipient (RLS-restricted — see `game_group_event` for another
/// per-recipient table of this shape). A whisper
/// makes TWO rows: the incoming line to the target (`is_inform = false` → "X whispers: …") and the
/// echo to the sender (`is_inform = true` → "To X: …"). `other_guid` is the OTHER party (the sender
/// for the target's row, the target for the echo) — the client resolves the name via NAME_QUERY.
/// Reaped by the shared event GC. [event]
#[table(accessor = game_whisper_event, public, index(accessor = by_recipient, btree(columns = [recipient_identity])))]
pub struct WhisperEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub recipient_identity: Identity,
    pub other_guid: u64,
    pub is_inform: bool,
    pub message: String,
    pub created_at: Timestamp,
    /// The recipient's CHARACTER GUID (issue #22, whisper slice). END-appended + defaulted, so this
    /// is an additive auto-migration and every pre-#22 row reads back as 0.
    ///
    /// `recipient_identity` cannot address a recipient on REALM-CORE: an identity is minted per
    /// (account, database) by the node, so the identity a player holds on a world shard names nobody
    /// on the directory database — and realm-core has no `game_character` rows to resolve one from in
    /// the first place. A guid is the one realm-wide name a character has. The identity column stays
    /// exactly as it was and still drives the per-player RLS on a world shard; this column is what
    /// the gateway's realm-core whisper relay filters on (it reads through the owner token, which
    /// bypasses RLS, and self-filters per session — the coordinator-relay law of 277/279, and the
    /// same shape `game_group_event.recipient_guid` took in the group slice).
    // The u64 default MUST be typed: a bare `0` encodes as 4 bytes and `publish` rejects the
    // migration with "data too short for u64: Expected 8, given 4" (world.rs:127 records the rule,
    // and `game_group_event.recipient_guid` shipped green with the untyped form an hour before it
    // blocked a deploy). Nothing in `cargo test`/`cargo check` validates default-value encoding.
    #[default(0u64)]
    pub recipient_guid: u64,
}

/// A connection drains only the whispers addressed to it (same RLS shape as movement relays).
#[client_visibility_filter]
const WHISPER_EVENT_RLS: Filter =
    Filter::Sql("SELECT * FROM game_whisper_event WHERE recipient_identity = :sender");

/// Whisper (`CMSG_MESSAGECHAT` with the Whisper type): deliver `message` privately to the player named
/// `target_name`, plus an echo to the sender. The target must be online; an unknown/offline target is
/// a clean `Err` the gateway turns into `SMSG_CHAT_PLAYER_NOT_FOUND`. Name match is case-insensitive
/// (vanilla `/w bob` reaches "Bob"). Authorized via `ctx.sender()`.
///
/// **The SHARD plane** of #22's whisper slice: this reducer resolves the target inside the CALLING
/// database, which is exactly why a whisper could not cross a shard boundary, and it stays the only
/// path a single-database gateway ever takes (byte-identical to pre-#22 — same gates, same order,
/// same rows). A multi-database gateway routes to [`realm_whisper`] instead; the ROW SHAPE both
/// planes write is the one shared core below ([`whisper_rows`] + [`push_whisper`]), so the ignore
/// rule and the sender's echo cannot drift between them.
#[reducer]
pub fn send_whisper(
    ctx: &ReducerContext,
    target_name: String,
    message: String,
) -> Result<(), String> {
    let sender = entity_by_owner(ctx, ctx.sender())
        .ok_or_else(|| lyracore_shared::whisper::NOT_IN_WORLD.to_string())?;
    apply_send_whisper(ctx, sender, target_name, message)
}

/// The shard-plane whisper core, actor-explicit (#479) — everything [`send_whisper`] does after
/// resolving WHO spoke. Same split as [`apply_send_chat`]; `gw::gw_send_whisper` is the other entry.
pub(crate) fn apply_send_whisper(
    ctx: &ReducerContext,
    sender: crate::WorldEntity,
    target_name: String,
    message: String,
) -> Result<(), String> {
    let text = normalized_message(&message).ok_or_else(|| "empty message".to_string())?;
    // Case-insensitive name match (vanilla `/w bob` reaches "Bob"). REFUSE verdict (issue #30): the
    // fenced `character_by_name` reads an in-transit character as ABSENT, so a whisper aimed at a
    // character mid-shard-hop falls into the existing not-found arm the gateway already maps to
    // SMSG_CHAT_PLAYER_NOT_FOUND — no new error string, no gateway edit.
    let target = crate::helpers::character_by_name(ctx, &target_name)
        .ok_or_else(|| lyracore_shared::whisper::no_player_named(&target_name))?;
    if !target.online {
        // No offline whispering in vanilla — the gateway maps this to SMSG_CHAT_PLAYER_NOT_FOUND too.
        return Err(lyracore_shared::whisper::player_is_offline(&target_name));
    }
    // Ignore enforcement: if the TARGET has the sender on their ignore list, the
    // incoming line is silently dropped — no row for them, so no `SMSG_MESSAGECHAT` relay ever
    // reaches "the ignorer". The sender still gets their own echo (matches vanilla: whispering
    // someone who's ignoring you shows no error, it just never arrives).
    let sender_is_ignored = ctx
        .db
        .game_character_contact()
        .by_owner()
        .filter(&target.guid)
        .any(|c| c.is_ignore && c.target_guid == sender.guid);
    for (recipient_guid, other_guid, is_inform) in
        whisper_rows(sender.guid, target.guid, sender_is_ignored)
    {
        push_whisper(ctx, recipient_guid, other_guid, is_inform, text.clone());
    }
    Ok(())
}

/// The REALM-CORE plane of the whisper (issue #22, whisper slice): deliver `message` from
/// `sender_guid` to `target_guid`, both named by GUID because a name means nothing here.
///
/// **Operator-gated, and it has to be** — the same trust boundary `realm_group_op` sits on. It takes
/// the sending character's guid as an argument instead of deriving it from `ctx.sender()`, because
/// realm-core has no live entity to derive one from, so a client that could call it would whisper
/// (and, worse, be *seen* to whisper) as anybody in the realm. The gateway is the only caller, it
/// holds the coordinator (operator) token, and it passes the guid it already authenticated for that
/// socket (`InWorld::self_guid`).
///
/// **Three gates ran in the gateway before this call**, and they had to: the directory database holds
/// neither characters, nor live entities, nor contact rows.
/// - does the target EXIST (realm-wide name → guid), and is it ONLINE — the two reads that made a
///   cross-boundary whisper impossible, answered across every connected shard;
/// - is the SENDER in world;
/// - and `sender_is_ignored` — the TARGET's ignore list, which lives with the target's character on
///   whichever shard holds it. Passing it as an argument keeps the RULE here (an ignored sender still
///   gets their echo, and is told nothing) while the DATA is read where it exists.
///
/// The rows are the same [`whisper_rows`] core the shard plane writes, so an ignored whisper behaves
/// identically on both planes. Addressing: [`push_whisper`] resolves each recipient's bound identity
/// from its character row and falls back to `Identity::ZERO` here (no rows on realm-core), which no
/// client's `:sender` can equal — so the RLS filter hides these rows from every player connection and
/// the owner-token coordinator relay is their only reader.
#[reducer]
pub fn realm_whisper(
    ctx: &ReducerContext,
    sender_guid: u64,
    target_guid: u64,
    message: String,
    sender_is_ignored: bool,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let text = normalized_message(&message).ok_or_else(|| "empty message".to_string())?;
    for (recipient_guid, other_guid, is_inform) in
        whisper_rows(sender_guid, target_guid, sender_is_ignored)
    {
        push_whisper(ctx, recipient_guid, other_guid, is_inform, text.clone());
    }
    Ok(())
}

/// The rows one whisper produces, as `(recipient_guid, other_guid, is_inform)` — the whole delivery
/// rule of a whisper, extracted so it is the SAME on the shard plane and on realm-core (issue #22).
///
/// Two rows, because a whisper is two chat lines: the incoming line to the target
/// ("<sender> whispers: …") and the echo to the sender ("To <target>: …"). The echo is unconditional
/// and the incoming line is not: an ignored sender's line is dropped at the source, so no
/// `SMSG_MESSAGECHAT` can reach the ignorer, and the sender is told nothing (vanilla shows no error —
/// the whisper simply never arrives). Order is incoming-then-echo, matching pre-#22 insert order.
///
/// Pure — unit-tested without a `ReducerContext`, which is the only way either plane's delivery rule
/// is testable in this crate at all.
pub(crate) fn whisper_rows(
    sender_guid: u64,
    target_guid: u64,
    sender_is_ignored: bool,
) -> Vec<(u64, u64, bool)> {
    let mut rows = Vec::with_capacity(2);
    if !sender_is_ignored {
        rows.push((target_guid, sender_guid, false));
    }
    rows.push((sender_guid, target_guid, true));
    rows
}

/// Insert one whisper row, addressed BOTH ways (issue #22, whisper slice): by `recipient_guid` (the
/// realm-wide name, which the gateway's realm-core relay self-filters on) and by the recipient's bound
/// `recipient_identity` (the per-player RLS a world shard delivers through, unchanged).
///
/// The identity is resolved from the recipient's character row, exactly as `group::push_event` does,
/// and falls back to [`Identity::ZERO`] when this database has no such row — which is always the case
/// on realm-core. ZERO matches no client (`:sender` is never ZERO), so a realm-core whisper row is
/// visible to the owner-token coordinator alone.
///
/// On a world shard this is byte-identical to the two inline inserts it replaces: the target's row
/// carried `target.owner_identity`, which is the same column read here, and the sender's echo carried
/// `ctx.sender()` — the identity `establish_session` stamped onto every character row of that account
/// and the one `entity_by_owner` just matched the sender's live entity by.
fn push_whisper(
    ctx: &ReducerContext,
    recipient_guid: u64,
    other_guid: u64,
    is_inform: bool,
    message: String,
) {
    let bound = ctx
        .db
        .game_character()
        .guid()
        .find(recipient_guid)
        .map(|c| c.owner_identity);
    ctx.db.game_whisper_event().insert(WhisperEvent {
        id: 0,
        recipient_identity: crate::helpers::event_recipient_identity(bound),
        other_guid,
        is_inform,
        message,
        created_at: ctx.timestamp,
        recipient_guid,
    });
}

// ===========================================================================================
//  Party chat [event] — work-item 199, RLS-scoped to CURRENT group members (the game_group_event
//  pattern), NOT proximity-based
// ===========================================================================================

/// `/p` (`CMSG_MESSAGECHAT` Party): deliver `text` to every OTHER member of the caller's group, plus
/// an echo to the caller — vanilla server-echoes the speaker's own party line back to them (unlike
/// say/yell's broadcast, where the speaker hears their own line via the SAME row every observer
/// gets; a per-recipient relay needs its own explicit echo row, exactly like `send_whisper` above).
/// Rides the group system's existing per-recipient `game_group_event` relay
/// (`kind = group_event_kind::PARTY_CHAT`) instead of a new gateway-subscribed table — identical
/// shape (one recipient, RLS-scoped by identity, a small payload) to every other kind that table
/// already carries (INVITE/LIST/DECLINE/DESTROYED, the work-item 187 roll/master-loot/money-share
/// kinds). Bounds-checked identically to `send_chat` (trim + [`MAX_CHAT_LEN`]-cap, empty rejected).
/// The caller must currently be in a group — `send_whisper`'s "no such/offline target" analog here
/// is simply "not in a group" ([`lyracore_shared::group::err::NOT_IN_GROUP`], the SAME shared-contract
/// string `group_leave`/`group_uninvite` already return for this exact condition), which the gateway
/// maps to `SMSG_PARTY_COMMAND_RESULT(NotInGroup)` — the standard "You aren't in a party" line.
/// Unlike `send_chat`, this reducer takes no `language` argument: like whispers, party lines aren't
/// language-filtered (always Universal on the wire), so there is nothing to thread through.
#[reducer]
pub fn party_chat(ctx: &ReducerContext, text: String) -> Result<(), String> {
    let sender =
        entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "speaker not in world".to_string())?;
    apply_party_chat(ctx, sender, text)
}

/// The party-chat core, actor-explicit (#479) — same split as [`apply_send_chat`].
pub(crate) fn apply_party_chat(
    ctx: &ReducerContext,
    sender: crate::WorldEntity,
    text: String,
) -> Result<(), String> {
    let message = normalized_message(&text).ok_or_else(|| "empty message".to_string())?;
    let membership = crate::group::group_of(ctx, sender.guid)
        .ok_or_else(|| group_err::NOT_IN_GROUP.to_string())?;
    let payload = lyracore_shared::group::encode_party_chat(&message);
    let member_guids: Vec<u64> = crate::group::members_of(ctx, membership.group_id)
        .into_iter()
        .map(|m| m.character_guid)
        .collect();
    for other in party_chat_other_recipients(sender.guid, &member_guids) {
        crate::group::push_event(
            ctx,
            other,
            group_event_kind::PARTY_CHAT,
            sender.guid,
            payload.clone(),
        );
    }
    // The echo to the sender (vanilla server-echoes party lines back to the speaker's own client) —
    // deliberately a SEPARATE push outside the loop above (which excludes the sender by design), not
    // folded into "every member incl. self", so the two audiences (others vs. the echo) stay
    // independently readable/testable — the [`party_chat_other_recipients`] pure function only ever
    // has to answer "who ELSE", never "who, including me".
    crate::group::push_event(
        ctx,
        sender.guid,
        group_event_kind::PARTY_CHAT,
        sender.guid,
        payload,
    );
    Ok(())
}

/// The OTHER group members who get `party_chat`'s per-recipient event row (every member of
/// `members` except `sender_guid`) — the sender gets a SEPARATE explicit echo row (see
/// `party_chat`), so this deliberately excludes them rather than the caller having to de-dupe a
/// combined list. Pure — unit-tested without a `ReducerContext`.
pub(crate) fn party_chat_other_recipients(sender_guid: u64, members: &[u64]) -> Vec<u64> {
    members
        .iter()
        .copied()
        .filter(|&g| g != sender_guid)
        .collect()
}

// ===========================================================================================
//  Friends / ignore list [entity] — durable contact rows
// ===========================================================================================

/// Vanilla client caps: the social pane won't render past 50 friends / 25 ignored names.
pub const MAX_FRIENDS: usize = 50;
pub const MAX_IGNORED: usize = 25;

/// One contact-list row: `owner_guid`'s friend OR ignore entry for `target_guid` (same table for
/// both — `is_ignore` picks the list, mirroring how `chat_type` picks Say vs Yell). RLS-scoped to
/// the owning character (like `game_player_skill`/`game_player_reputation`) — a connection only
/// ever sees its OWN contacts; the gateway resolves an online friend's presence/level/class/zone via
/// the privileged coordinator read (same trick `online_players` uses for `/who`). [entity]
#[table(accessor = game_character_contact, public, index(accessor = by_owner, btree(columns = [owner_guid])))]
pub struct ContactEntry {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub owner_guid: u64,
    pub owner_identity: Identity,
    pub target_guid: u64,
    pub is_ignore: bool, // false = friend, true = ignore
}

/// A player connection sees only its own contact rows (mirrors the skill/reputation RLS filters).
#[client_visibility_filter]
const CONTACT_RLS: Filter =
    Filter::Sql("SELECT * FROM game_character_contact WHERE owner_identity = :sender");

// Character-owned sweeps: a character's OWN contact rows are deleted with it; rows
// OTHER characters hold that point AT the deleted guid (a stale friend/ignore reference) are swept
// too — `character_guid` here plays double duty as both "owner" (the delete-marker's contract) and
// "possible target elsewhere", so the delete sweep does both passes.
crate::character_owned!(delete, fn sweep_delete_game_character_contact(ctx, character_guid) {
    let contacts = ctx.db.game_character_contact();
    for r in contacts.by_owner().filter(&character_guid).collect::<Vec<_>>() {
        contacts.id().delete(r.id);
    }
    for r in contacts.iter().filter(|c| c.target_guid == character_guid).collect::<Vec<_>>() {
        contacts.id().delete(r.id);
    }
});
// CROSS-DATABASE transport (issue #19): the character's OWN friend/ignore rows travel with it (`id`
// re-minted — surrogate PK). Rows where it is the TARGET belong to OTHER characters and stay where
// their owner is, which is why only `by_owner` is exported: a friend list is per-owner state, and
// copying someone else's row would fork it.
crate::character_owned!(transfer, fn sweep_transfer_game_character_contact(ctx, character_guid, io) {
    table = game_character_contact,
    by = by_owner,
    remint = id,
});
crate::character_owned!(restamp, fn sweep_restamp_game_character_contact(ctx, character_guid, identity) {
    let contacts = ctx.db.game_character_contact();
    for mut r in contacts.by_owner().filter(&character_guid).collect::<Vec<_>>() {
        if r.owner_identity != identity {
            r.owner_identity = identity;
            contacts.id().update(r);
        }
    }
});

/// Shared add path for `add_friend`/`add_ignore`: `target_guid` is resolved by the GATEWAY (name →
/// guid, same lookup `/who` uses) before the reducer is called, so this only re-validates
/// server-side (never trusts the caller) — reject self, an unknown guid, a duplicate, or a full list.
pub(crate) fn add_contact(
    ctx: &ReducerContext,
    sender: crate::WorldEntity,
    target_guid: u64,
    is_ignore: bool,
) -> Result<(), String> {
    if target_guid == sender.guid {
        return Err("cannot add yourself".to_string());
    }
    if ctx.db.game_character().guid().find(target_guid).is_none() {
        return Err("no such player".to_string());
    }
    let contacts = ctx.db.game_character_contact();
    let existing: Vec<_> = contacts.by_owner().filter(&sender.guid).collect();
    if existing
        .iter()
        .any(|c| c.target_guid == target_guid && c.is_ignore == is_ignore)
    {
        return Err("already added".to_string());
    }
    let cap = if is_ignore { MAX_IGNORED } else { MAX_FRIENDS };
    if existing.iter().filter(|c| c.is_ignore == is_ignore).count() >= cap {
        return Err("list full".to_string());
    }
    contacts.insert(ContactEntry {
        id: 0,
        owner_guid: sender.guid,
        // The OWNER's own binding, not `ctx.sender()` — identical on the sender path, and correct
        // on the gateway path where `ctx.sender()` is the shared connection's operator identity.
        owner_identity: sender.owner_identity,
        target_guid,
        is_ignore,
    });
    Ok(())
}

/// Shared remove path for `del_friend`/`del_ignore`: deletes the caller's own row for `target_guid`
/// in the given list, or a clean `Err` if it isn't there (idempotent double-remove from the client).
pub(crate) fn remove_contact(
    ctx: &ReducerContext,
    sender: crate::WorldEntity,
    target_guid: u64,
    is_ignore: bool,
) -> Result<(), String> {
    let contacts = ctx.db.game_character_contact();
    let row = contacts
        .by_owner()
        .filter(&sender.guid)
        .find(|c| c.target_guid == target_guid && c.is_ignore == is_ignore)
        .ok_or_else(|| "not on that list".to_string())?;
    contacts.id().delete(row.id);
    Ok(())
}

/// `CMSG_ADD_FRIEND`: add `target_guid` (gateway-resolved from the typed name) to the caller's
/// friend list.
#[reducer]
pub fn add_friend(ctx: &ReducerContext, target_guid: u64) -> Result<(), String> {
    let sender = entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "not in world".to_string())?;
    add_contact(ctx, sender, target_guid, false)
}

/// `CMSG_DEL_FRIEND`: remove `target_guid` from the caller's friend list.
#[reducer]
pub fn del_friend(ctx: &ReducerContext, target_guid: u64) -> Result<(), String> {
    let sender = entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "not in world".to_string())?;
    remove_contact(ctx, sender, target_guid, false)
}

/// `CMSG_ADD_IGNORE`: add `target_guid` (gateway-resolved from the typed name) to the caller's
/// ignore list.
#[reducer]
pub fn add_ignore(ctx: &ReducerContext, target_guid: u64) -> Result<(), String> {
    let sender = entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "not in world".to_string())?;
    add_contact(ctx, sender, target_guid, true)
}

/// `CMSG_DEL_IGNORE`: remove `target_guid` from the caller's ignore list.
#[reducer]
pub fn del_ignore(ctx: &ReducerContext, target_guid: u64) -> Result<(), String> {
    let sender = entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "not in world".to_string())?;
    remove_contact(ctx, sender, target_guid, true)
}

// ===========================================================================================
//  Random roll [event] — `/roll` broadcast (MSG_RANDOM_ROLL)
// ===========================================================================================

/// A `/roll` result to fan out as `MSG_RANDOM_ROLL_Server`. Public broadcast (no RLS), like
/// `game_chat_event`; the gateway relays each `on_insert`. Reaped by the shared event GC. [event]
#[table(
    accessor = game_roll_event,
    public,
    // perf catalog 2.3: AOI-box scoping instead of a global `SELECT *`.
    index(accessor = by_grid, btree(columns = [map_id, instance_id, grid_x, grid_y]))
)]
pub struct RollEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub roller_guid: u64,
    pub min_roll: u32,
    pub max_roll: u32,
    pub result: u32,
    pub created_at: Timestamp,
    // --- AOI columns (perf catalog 2.3), END-appended + TYPED defaults (a bare `0` on a u64
    // encodes as 4 bytes and fails the publish). Stamped from the actor via `helpers::grid_of`;
    // (0,0,0,0) means "no live actor", which matches no box and is correctly never delivered.
    #[default(0u32)]
    pub map_id: u32,
    #[default(0u64)]
    pub instance_id: u64,
    #[default(0i32)]
    pub grid_x: i32,
    #[default(0i32)]
    pub grid_y: i32,
}

/// Handle `MSG_RANDOM_ROLL` (client → server): pick a server-side random value in
/// `[min_roll, max_roll]` (inclusive) and broadcast the result. Clamps inverted ranges
/// (`min > max`) by swapping and caps both ends at 10 000 (the vanilla client ceiling).
/// Authorized via `ctx.sender()`. Dead players can roll (vanilla allows it).
#[reducer]
pub fn send_roll(ctx: &ReducerContext, min_roll: u32, max_roll: u32) -> Result<(), String> {
    let roller =
        entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "roller not in world".to_string())?;
    apply_send_roll(ctx, roller, min_roll, max_roll)
}

/// The `/roll` core, actor-explicit (#479) — same split as [`apply_send_chat`].
pub(crate) fn apply_send_roll(
    ctx: &ReducerContext,
    roller: crate::WorldEntity,
    min_roll: u32,
    max_roll: u32,
) -> Result<(), String> {
    let (lo, hi) = normalized_roll_range(min_roll, max_roll);
    // Server-side RNG via ctx.random (like combat::roll_money) — NOT a timestamp-seeded LCG:
    // two rolls landing in the same microsecond used to be affinely related regardless of
    // roller guid, which matters because /roll is the primitive players use to settle loot
    // disputes.
    let range = hi - lo + 1;
    let result = lo + ctx.random::<u32>() % range;
    // perf catalog 2.3: AOI address of the actor — `roller` is already the live entity in hand, so
    // this stamps directly off it (`entity_addr`) instead of paying a redundant `grid_of` PK lookup.
    let (map_id, instance_id, grid_x, grid_y) = crate::helpers::entity_addr(&roller);
    ctx.db.game_roll_event().insert(RollEvent {
        id: 0,
        roller_guid: roller.guid,
        min_roll: lo,
        max_roll: hi,
        result,
        created_at: ctx.timestamp,
        map_id,
        instance_id,
        grid_x,
        grid_y,
    });
    Ok(())
}

/// Normalize a client `/roll` range to the broadcast `(lo, hi)`: swap an inverted pair
/// (`min > max`) and cap BOTH ends at 10 000 — the vanilla client ceiling. The clamp
/// `send_roll` applies before drawing the result, extracted so it's unit-testable without a
/// `ReducerContext`. `lo <= hi` always holds on the way out. Pure.
pub(crate) fn normalized_roll_range(min_roll: u32, max_roll: u32) -> (u32, u32) {
    // Vanilla caps roll at 10 000; invert if client sends min > max.
    const ROLL_MAX: u32 = 10_000;
    let lo = min_roll.min(max_roll).min(ROLL_MAX);
    let hi = min_roll.max(max_roll).min(ROLL_MAX);
    (lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_say_and_yell_are_supported() {
        assert!(is_supported_chat_type(CHAT_SAY));
        assert!(is_supported_chat_type(CHAT_YELL));
        assert!(!is_supported_chat_type(2)); // party/guild/whisper/etc. rejected
        assert!(!is_supported_chat_type(255));
    }

    #[test]
    fn message_is_trimmed_capped_and_empty_rejected() {
        assert_eq!(normalized_message("  hello  ").as_deref(), Some("hello"));
        assert_eq!(normalized_message(""), None);
        assert_eq!(normalized_message("   \t  "), None);
        let long: String = "a".repeat(1000);
        assert_eq!(
            normalized_message(&long).unwrap().chars().count(),
            MAX_CHAT_LEN
        );
    }

    #[test]
    fn roll_range_swaps_an_inverted_pair_and_caps_both_ends_at_10000() {
        // The default /roll is passed through untouched.
        assert_eq!(normalized_roll_range(1, 100), (1, 100));
        // An inverted pair (min > max) swaps rather than erroring or underflowing the range width.
        assert_eq!(normalized_roll_range(100, 1), (1, 100));
        // A degenerate single-value range survives (the draw is then that value).
        assert_eq!(normalized_roll_range(5, 5), (5, 5));
        assert_eq!(normalized_roll_range(0, 0), (0, 0));
        // The 10 000 client ceiling caps the high end alone, or both ends of a huge range.
        assert_eq!(normalized_roll_range(1, 2_000_000), (1, 10_000));
        assert_eq!(
            normalized_roll_range(3_000_000, 2_000_000),
            (10_000, 10_000)
        );
        // Exactly at the ceiling is legal, not clipped below it.
        assert_eq!(normalized_roll_range(1, 10_000), (1, 10_000));
    }

    // ---- Party chat (work-item 199) ----

    #[test]
    fn party_chat_routes_to_every_other_member_and_excludes_the_sender() {
        // A 3-person party: the sender (20) is excluded from its own "other recipients" list — the
        // reducer gives the sender a SEPARATE echo row instead (see `party_chat`'s doc).
        let members = [10u64, 20, 30];
        let mut others = party_chat_other_recipients(20, &members);
        others.sort_unstable();
        assert_eq!(
            others,
            vec![10, 30],
            "the sender must never appear among the OTHER recipients"
        );
    }

    #[test]
    fn party_chat_other_recipients_is_empty_when_the_sender_is_the_only_member() {
        // Degenerate case (shouldn't happen in practice — this codebase disbands below 2 members —
        // but the pure filter must not panic or wrongly include the sender).
        assert_eq!(party_chat_other_recipients(7, &[7]), Vec::<u64>::new());
    }

    // ---- Whisper delivery (issue #22, whisper slice) ----

    /// The delivery rule both planes share. A whisper is TWO lines, and which of them exists is the
    /// whole of the ignore rule: the echo is unconditional (the sender is never told they were
    /// ignored — vanilla shows no error) and the incoming line is dropped at the source, so no
    /// relay can reach the ignorer.
    #[test]
    fn a_whisper_writes_the_incoming_line_and_the_senders_echo() {
        // (recipient, other, is_inform) — incoming first, echo second, matching pre-#22 order.
        assert_eq!(
            whisper_rows(10, 20, false),
            vec![(20, 10, false), (10, 20, true)]
        );
    }

    #[test]
    fn an_ignored_sender_loses_the_incoming_line_and_keeps_their_own_echo() {
        let rows = whisper_rows(10, 20, true);
        assert_eq!(
            rows,
            vec![(10, 20, true)],
            "the ignorer must get NO row (a row is a relayed SMSG_MESSAGECHAT), and the sender must \
             still see their own \"To X:\" echo — vanilla reports nothing to a sender being ignored"
        );
        assert!(
            !rows.iter().any(|&(recipient, ..)| recipient == 20),
            "no row may be addressed to the ignoring target"
        );
    }

    /// A self-whisper (`/w <own name>`) is legal in vanilla and produces both lines, both addressed
    /// to the same player — the degenerate case a `recipient != sender` filter would silently break.
    #[test]
    fn a_self_whisper_still_produces_both_lines() {
        assert_eq!(
            whisper_rows(10, 10, false),
            vec![(10, 10, false), (10, 10, true)]
        );
    }

    // ---- `realm_whisper`'s two unreachable decisions (issue #22, whisper slice) ----
    //
    // A reducer body needs a live `ReducerContext`, so the two below cannot be EXECUTED by a test in
    // this crate — which is why they are scanned. Same technique, and the same reason, as
    // `group.rs`'s pair for `realm_group_op`.

    /// The `//`-stripped body of `signature`'s function — assert on CODE, never on the prose beside
    /// it. Shared with every other file's copy of this scan as [`crate::test_scan::code_of`]
    /// (issue #64 — this used to be six near-identical, drifted-apart copies).
    use crate::test_scan::code_of;

    /// **The operator gate is the entire authorization of the realm-core whisper plane.**
    ///
    /// `realm_whisper` takes the SENDING character's guid as an argument rather than deriving it from
    /// `ctx.sender()` (realm-core has no live entity to derive one from), so without the gate any
    /// identity that can reach the node can whisper any player in the realm *as* any other player —
    /// an impersonation primitive, not merely an unauthorized write.
    ///
    /// Asserted as the FIRST STATEMENT of the body, not merely present in it. A bare `contains` is
    /// satisfied by a gate that never runs: wrapping the line in `if false { … }` — this batch's own
    /// documented tripwire defeat — left all 521 module tests green with the reducer completely
    /// ungated, and so would `let _ = crate::helpers::require_operator(ctx);` or a `return Ok(())`
    /// placed above it. Anchoring to the opening brace makes every one of those visible, and it is the
    /// same anchoring the whisper relay's own scan uses in `stdb/subscriptions.rs`.
    #[test]
    fn the_realm_whisper_reducer_is_operator_gated() {
        let body = code_of(include_str!("chat.rs"), "pub fn realm_whisper(");
        let normalized: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            normalized.starts_with("{ crate::helpers::require_operator(ctx)?;"),
            "`realm_whisper` no longer OPENS with the operator gate. It takes the SENDER's guid as an \
             argument, so the gate is the only thing between an arbitrary connection and whispering \
             as anybody in the realm — and a gate that is present but neutralized (wrapped in \
             `if false`, `let _ =`, or preceded by an early return) is no gate. Body was:\n{body}"
        );
    }

    /// **Both of a whisper row's addresses, pinned.** A whisper is delivered by exactly two columns
    /// and there is no `ReducerContext` in this crate to observe either, so a wrong one is a silent
    /// total delivery failure with every suite green — verified by mutation, twice:
    ///
    /// - `recipient_guid: 0` (instead of the parameter) → every CROSS-DATABASE whisper is written on
    ///   realm-core and delivered to nobody: the gateway's realm relay self-filters on this column and
    ///   0 is no session's guid.
    /// - a constant `recipient_identity` → every SINGLE-DATABASE whisper stops arriving: the per-player
    ///   RLS filter is `recipient_identity = :sender`, so the row becomes invisible to its own
    ///   recipient. (`Identity::ZERO` is the deliberate FALLBACK for a database with no such character
    ///   — realm-core — which is exactly why hardcoding it looks so innocent.)
    #[test]
    fn a_whisper_row_is_addressed_by_recipient_guid_and_by_the_recipients_bound_identity() {
        let body = code_of(include_str!("chat.rs"), "fn push_whisper(");
        let n: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
        // The needle deliberately omits the `game_character()` accessor: spelling it here would count
        // as a raw character lookup in THIS file to the `character_fence_tripwire` scanner (which
        // reads source, not semantics), and inflating that budget to cover a test string would weaken
        // a ratchet that exists to catch real reads.
        assert!(
            n.contains("let bound = ctx .db .game_char"),
            "`push_whisper` no longer reads the recipient's character row for its identity. \
             Body was:\n{body}"
        );
        assert!(
            n.contains(".guid() .find(recipient_guid) .map(|c| c.owner_identity);"),
            "`push_whisper` no longer resolves the RECIPIENT's own bound identity. Resolving anyone \
             else's (the sender's, `other_guid`'s) hands a private chat line to the wrong \
             connection. Body was:\n{body}"
        );
        assert!(
            n.contains("recipient_identity: crate::helpers::event_recipient_identity(bound),"),
            "`push_whisper` no longer addresses the row to the recipient's bound identity via the \
             shared ZERO-fallback helper. A constant there makes every whisper on a single-database \
             gateway invisible to its recipient (the RLS filter is `recipient_identity = :sender`). \
             Body was:\n{body}"
        );
        assert!(
            n.contains("created_at: ctx.timestamp, recipient_guid, });"),
            "`push_whisper` no longer stamps `recipient_guid` from its argument. The gateway's \
             realm-core relay self-filters on this column, so a literal there delivers every \
             cross-database whisper to nobody — and nothing else in either suite can see it. \
             Body was:\n{body}"
        );
        // ONE binding, because a `contains` proves presence and never exclusivity. Appending
        // `let bound = None;` after the pinned read leaves all four needles above intact, compiles
        // clean, adds no raw character lookup for the fence tripwire to notice — and addresses EVERY
        // whisper row to `Identity::ZERO`, which is survivor #2 of this slice's own mutation list
        // (every single-database whisper invisible to its recipient) wearing a scan-defeating
        // disguise. Verified: all 521 module tests green. The count is the fence.
        assert_eq!(
            n.matches("let bound").count(),
            1,
            "`push_whisper` binds `bound` more than once. A SHADOWED rebind keeps every needle above \
             satisfied while the row is addressed to somebody else — or to nobody. Body was:\n{body}"
        );
    }

    /// **Both planes drive the shared core, in the declared argument order.**
    ///
    /// `whisper_rows(sender, target, ignored)` is the only thing that decides who receives a whisper,
    /// and its two call sites are the two planes. Neither is executable here, and both admit a silent
    /// mutation the rest of the tree cannot see (verified — each left all suites green):
    ///
    /// - the flag hardcoded to `false` on the realm plane → every ignored whisper delivered, and the
    ///   verdict the gateway went to another database to read thrown away;
    /// - the guids SWAPPED on the shard plane → the "X whispers:" line goes to the sender and the
    ///   "To X:" echo to the target, so both parties see the conversation inverted.
    #[test]
    fn both_whisper_planes_drive_the_shared_row_core_in_its_declared_order() {
        let src = include_str!("chat.rs");
        for (plane, signature, call) in [
            (
                "the REALM-CORE plane",
                "pub fn realm_whisper(",
                "whisper_rows(sender_guid, target_guid, sender_is_ignored)",
            ),
            (
                // #479 factored the shard plane's body out of the `send_whisper` reducer into the
                // actor-explicit core both entries (sender + `gw_send_whisper`) delegate to — the
                // pin follows the body, which is where the delivery rule actually lives.
                "the SHARD plane",
                "pub(crate) fn apply_send_whisper(",
                "whisper_rows(sender.guid, target.guid, sender_is_ignored)",
            ),
        ] {
            let body = code_of(src, signature);
            let normalized: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(
                normalized.contains(call),
                "{plane} no longer drives `{call}`. The shared core is the whole delivery rule of a \
                 whisper — who gets the line, who gets the echo, and whether an ignored sender is \
                 dropped — and a swapped or hardcoded argument here inverts or leaks a private \
                 conversation with nothing else in either suite able to see it. Body was:\n{body}"
            );
        }
    }

    #[test]
    fn party_chat_other_recipients_is_order_preserving_and_sender_agnostic_to_position() {
        // The sender can be anywhere in the roster (not just first/last) and is filtered regardless
        // of position; everyone else's relative order survives untouched.
        assert_eq!(party_chat_other_recipients(2, &[1, 2, 3, 4]), vec![1, 3, 4]);
        assert_eq!(party_chat_other_recipients(1, &[1, 2, 3]), vec![2, 3]);
        assert_eq!(party_chat_other_recipients(3, &[1, 2, 3]), vec![1, 2]);
    }
}
