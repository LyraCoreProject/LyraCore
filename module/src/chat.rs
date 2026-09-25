//! Social tier: say, yell, `/e`, text emotes (`/dance`, `/wave`, …), contacts and `/roll`. A
//! player's `CMSG_MESSAGECHAT` or `CMSG_TEXT_EMOTE` becomes a broadcast event row that the Gateway
//! turns into `SMSG_MESSAGECHAT` / `SMSG_TEXT_EMOTE` (+ `SMSG_EMOTE` animation). Say, yell and `/e`
//! rows carry no range: the Gateway scopes them to the speaker's surroundings when it relays them.
//! Whisper, party and channel chat are not here. They are Realm Chat Lines (`crate::realm_chat`),
//! committed on Realm-core with their whole audience. Chat Channels themselves live in
//! `crate::channel`, and Away Status in `crate::away`. [event]

use spacetimedb::{table, Identity, ReducerContext, Table, Timestamp};

use lyracore_shared::social::ContactRefusal;

// `game_character_contact` is defined further down in THIS module — its `#[table]` accessor
// (`game_character_contact`) is generated there, so it's in scope for `add_friend`/etc. without a
// `use`.

/// `game_chat_event.chat_type` discriminants for the broadcast chat types this table carries,
/// re-exported from the shared source of truth both crates read
/// ([`lyracore_shared::chat::broadcast_chat`]). Whisper and party are not `game_chat_event` rows:
/// they ride `game_realm_chat_event`, whose per-recipient shape this broadcast table cannot hold.
pub const CHAT_SAY: u8 = lyracore_shared::chat::broadcast_chat::SAY;
pub const CHAT_YELL: u8 = lyracore_shared::chat::broadcast_chat::YELL;
/// Creature-authored text emote (`CHAT_TYPE_TEXT_EMOTE` on the source wire). It uses the same
/// broadcast row as Say and Yell; the gateway maps the discriminant to `CHAT_MSG_MONSTER_EMOTE`.
/// EventAI is its only source — [`apply_send_chat`] refuses it from a Character.
pub const CHAT_TEXT_EMOTE: u8 = lyracore_shared::chat::broadcast_chat::CREATURE_TEXT_EMOTE;
/// A Character's `/e` custom emote. [`apply_send_chat`] is its only source and always stores it in
/// [`lyracore_shared::chat::language::UNIVERSAL`], whatever language byte the client sent
/// (cm:Player.cpp:16591-16599).
pub const CHAT_EMOTE: u8 = lyracore_shared::chat::broadcast_chat::EMOTE;

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
    pub chat_type: u8, // CHAT_SAY / CHAT_YELL / CHAT_TEXT_EMOTE / CHAT_EMOTE
    pub language: u8,  // vanilla Language discriminant, echoed back to clients
    pub message: String,
    pub created_at: Timestamp,
    /// The addressed unit for creature-authored Say/Yell. Zero for unaddressed and player chat.
    /// END-appended with a typed default so existing rows migrate safely.
    #[default(0u64)]
    pub target_guid: u64,
}

/// True for the creature broadcast chat types this slice relays. [pure]
pub fn is_supported_chat_type(chat_type: u8) -> bool {
    matches!(chat_type, CHAT_SAY | CHAT_YELL | CHAT_TEXT_EMOTE)
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

/// An addon-language payload exactly as the client sent it, length-capped like any line. Addons
/// frame their own data, so a trim would change it in transit; cmangos reads it raw and drops only
/// an empty one (cm:ChatHandler.cpp:306-312). [pure]
pub fn addon_payload(raw: &str) -> Option<String> {
    (!raw.is_empty()).then(|| raw.chars().take(MAX_CHAT_LEN).collect())
}

/// The say/yell/`/e` core, actor-explicit (stage 4a): everything the old sender-path `send_chat`
/// did after resolving WHO spoke, plus the player/EventAI boundary — a Character may say, yell or
/// `/e`, never submit the creature-only text emote. EMOTE is admitted ONLY here, never in
/// [`apply_send_chat_to`]: that function is EventAI's own entry, and its `chat_type` comes straight
/// off an imported `game_creature_ai_broadcast_text` row that Package import never range-checks —
/// widening ITS gate to admit EMOTE would let a malformed broadcast line masquerade as a
/// Character's `/e`, and the codec has no packet for a creature EMOTE row anyway. `gw::gw_send_chat`
/// delegates here.
pub(crate) fn apply_send_chat(
    ctx: &ReducerContext,
    sender: crate::WorldEntity,
    chat_type: u8,
    language: u8,
    message: String,
) -> Result<(), String> {
    admit_player_line(chat_type, sender.race(), language)?;
    write_chat_event(ctx, sender, 0, chat_type, language, message)
}

/// A Character may say, yell or `/e`, and only in a language its race knows. cmangos checks the
/// language skill before it looks at the kind (cm:ChatHandler.cpp:100-111), so `/e` passes the
/// same Gate even though it goes out in Universal. The Refusal is the `chat:*` tag the Gateway
/// answers with "You don't know that language". [pure]
fn admit_player_line(chat_type: u8, race: u8, language: u8) -> Result<(), String> {
    use lyracore_shared::chat::chat_kind;
    let kind = match chat_type {
        CHAT_SAY => chat_kind::SAY,
        CHAT_YELL => chat_kind::YELL,
        CHAT_EMOTE => chat_kind::EMOTE,
        _ => return Err(format!("unsupported chat type {chat_type}")),
    };
    lyracore_shared::chat::speakable_language(kind, race, u32::from(language))
        .map(drop)
        .map_err(|refusal| refusal.as_tag().to_string())
}

/// Creature-authored speech with its resolved addressed unit retained for the monster chat packet.
/// EventAI's only entry (`crate::creatures::eventai::relay`'s `RelayInstruction::Talk`, `engine`'s
/// `eventai_deliver_line`). The gate is `is_supported_chat_type` alone — see [`apply_send_chat`]'s
/// doc for why EMOTE must never widen it.
pub(crate) fn apply_send_chat_to(
    ctx: &ReducerContext,
    sender: crate::WorldEntity,
    target_guid: u64,
    chat_type: u8,
    language: u8,
    message: String,
) -> Result<(), String> {
    if !is_supported_chat_type(chat_type) {
        return Err(format!("unsupported chat type {chat_type}"));
    }
    write_chat_event(ctx, sender, target_guid, chat_type, language, message)
}

/// The row-write both entries above share once their own type gate has passed: the dead-guard
/// (matching vmangos for `/e`, vm:ChatHandler.cpp:360-361; cmangos has no such check for `/e`, but
/// the say/yell rule already followed vmangos, so one gate covers all three types either entry can
/// reach), message normalization, and the EMOTE-always-Universal rule (cm:Player.cpp:16594).
fn write_chat_event(
    ctx: &ReducerContext,
    sender: crate::WorldEntity,
    target_guid: u64,
    chat_type: u8,
    language: u8,
    message: String,
) -> Result<(), String> {
    // Vanilla: a dead/ghost player can't be heard via Say/Yell/`/e` (proximity chat). Whisper +
    // party/guild are NOT gated by death (and aren't routed here anyway).
    if sender.dead {
        return Err("dead players cannot speak".to_string());
    }
    let text = normalized_message(&message).ok_or_else(|| "empty message".to_string())?;
    let language = if chat_type == CHAT_EMOTE {
        lyracore_shared::chat::language::UNIVERSAL as u8
    } else {
        language
    };
    ctx.db.game_chat_event().insert(ChatEvent {
        id: 0,
        sender_guid: sender.guid,
        chat_type,
        language,
        message: text,
        created_at: ctx.timestamp,
        target_guid,
    });
    Ok(())
}

// ===========================================================================================
//  Retired shard-local chat channels. Chat Channels live on Realm-core (`crate::channel`). These
//  two tables stay in the schema, unwritten and unsubscribed, because dropping a table is a
//  destructive migration.
// ===========================================================================================

/// Retired: nothing writes it. [entity]
#[table(accessor = game_channel_member, public, index(accessor = by_channel, btree(columns = [channel])))]
pub struct ChannelMember {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub channel: String,
    pub character_guid: u64,
    pub owner_identity: Identity,
}

/// Retired: nothing writes it. [event]
#[table(accessor = game_channel_event, public)]
pub struct ChannelEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub channel: String,
    pub channel_display: String,
    pub sender_guid: u64,
    pub message: String,
    pub created_at: Timestamp,
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
///
/// The text-emote core, actor-explicit (stage 4a) — same split as [`apply_send_chat`].
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
    if sender.is_player() && target_guid != 0 {
        crate::creatures::eventai_on_receive_emote(ctx, target_guid, sender.guid, text_emote);
    }
    Ok(())
}

// ===========================================================================================
//  Retired whisper table. Whispers are Realm Chat Lines (`crate::realm_chat::realm_whisper`). The
//  table stays in the schema, unwritten and unsubscribed, because dropping a table is a destructive
//  migration.
// ===========================================================================================

/// Retired: nothing writes it. [event]
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
    #[default(0u64)]
    pub recipient_guid: u64,
}

/// A private System Message for one connected Character. The Gateway addresses the row by guid and
/// rechecks the recipient before enqueueing. Reaped by the shared event GC. [event]
#[table(accessor = game_system_message_event, public, index(accessor = by_recipient, btree(columns = [recipient_identity])))]
pub struct SystemMessageEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub recipient_identity: Identity,
    pub recipient_guid: u64,
    pub message: String,
    pub created_at: Timestamp,
}

/// Emit a private System Message for one online Character on this Shard. Package code calls this
/// through `actor::system_message`, so it needs no table or protocol knowledge.
#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn emit_system_message(
    ctx: &ReducerContext,
    character_guid: u64,
    message: String,
) -> Result<(), String> {
    let recipient = crate::helpers::character_by_guid(ctx, character_guid)
        .map(|character| (character.owner_identity, character.online));
    let event = prepare_system_message(character_guid, recipient, &message, ctx.timestamp)?;
    ctx.db.game_system_message_event().insert(event);
    Ok(())
}

fn prepare_system_message(
    character_guid: u64,
    recipient: Option<(Identity, bool)>,
    message: &str,
    created_at: Timestamp,
) -> Result<SystemMessageEvent, String> {
    let Some((recipient_identity, online)) = recipient else {
        return Err(format!(
            "system message recipient {character_guid} is not present on this Shard"
        ));
    };
    if !online {
        return Err(format!(
            "system message recipient {character_guid} is offline"
        ));
    }
    let message = normalized_message(message).ok_or_else(|| "empty system message".to_string())?;
    Ok(SystemMessageEvent {
        id: 0,
        recipient_identity,
        recipient_guid: character_guid,
        message,
        created_at,
    })
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
// CROSS-DATABASE transport: the character's OWN friend/ignore rows travel with it (`id`
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

/// Reducer edge for the contact list: only the tag crosses to the gateway, the detail stays here.
pub(crate) fn refused_contact(refusal: ContactRefusal, detail: &str) -> String {
    let tag = refusal.as_tag();
    spacetimedb::log::info!("contact refused {tag}: {detail}");
    tag.to_string()
}

/// Shared add path for `add_friend`/`add_ignore`, at the reducer edge: the core's typed Refusal
/// becomes its stable tag.
pub(crate) fn add_contact(
    ctx: &ReducerContext,
    sender: crate::WorldEntity,
    target_guid: u64,
    is_ignore: bool,
    target_race: Option<u8>,
) -> Result<(), String> {
    let owner_guid = sender.guid;
    add_contact_core(ctx, sender, target_guid, is_ignore, target_race).map_err(|refusal| {
        refused_contact(
            refusal,
            &format!("{owner_guid} could not add {target_guid}"),
        )
    })
}

/// `target_guid` is resolved by the GATEWAY (name → guid, same realm-wide lookup whisper and
/// group invite use) before the reducer is called — that resolution IS the existence Gate, so this
/// never re-checks for a durable row locally (a target on another Shard has none here). It still
/// re-validates everything server-side (never trusts the caller): self, a duplicate, a full list,
/// and — friends only — the other team. `target_race` is the Gateway-conveyed Speaker Fact for the
/// target; `None` for an ignore add, which has no faction rule.
fn add_contact_core(
    ctx: &ReducerContext,
    sender: crate::WorldEntity,
    target_guid: u64,
    is_ignore: bool,
    target_race: Option<u8>,
) -> Result<(), ContactRefusal> {
    if target_guid == sender.guid {
        return Err(ContactRefusal::AddSelf);
    }
    let refuses_enemy = !is_ignore
        && target_race
            .is_some_and(|race| !lyracore_shared::faction::same_team(sender.race(), race));
    if refuses_enemy {
        return Err(ContactRefusal::Enemy);
    }
    let contacts = ctx.db.game_character_contact();
    let existing: Vec<_> = contacts.by_owner().filter(&sender.guid).collect();
    if existing
        .iter()
        .any(|c| c.target_guid == target_guid && c.is_ignore == is_ignore)
    {
        return Err(ContactRefusal::AlreadyOnList);
    }
    let cap = if is_ignore { MAX_IGNORED } else { MAX_FRIENDS };
    if existing.iter().filter(|c| c.is_ignore == is_ignore).count() >= cap {
        return Err(ContactRefusal::ListFull);
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
/// in the given list, or [`ContactRefusal::NotOnList`] if it isn't there (idempotent double-remove
/// from the client).
pub(crate) fn remove_contact(
    ctx: &ReducerContext,
    sender: crate::WorldEntity,
    target_guid: u64,
    is_ignore: bool,
) -> Result<(), String> {
    let contacts = ctx.db.game_character_contact();
    let Some(row) = contacts
        .by_owner()
        .filter(&sender.guid)
        .find(|c| c.target_guid == target_guid && c.is_ignore == is_ignore)
    else {
        return Err(refused_contact(
            ContactRefusal::NotOnList,
            &format!("{} does not hold {target_guid}", sender.guid),
        ));
    };
    contacts.id().delete(row.id);
    Ok(())
}

// `CMSG_ADD_FRIEND` / `CMSG_DEL_FRIEND` / `CMSG_ADD_IGNORE` / `CMSG_DEL_IGNORE` all land on
// `add_contact`/`remove_contact` above via `gw::gw_add_friend`/`gw_del_friend`/`gw_add_ignore`/
// `gw_del_ignore` (the sender-path wrappers are gone).

// ===========================================================================================
//  Random roll: the range rule and the retired roll table
// ===========================================================================================

/// The former `/roll` result row. `/roll` is a Group Broadcast now (`group::random_roll_on`), so
/// nothing writes this table. A live table cannot be dropped, so it stays, empty, and the event
/// GC still reaps it. [event]
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
    // encodes as 4 bytes and fails the publish).
    #[default(0u32)]
    pub map_id: u32,
    #[default(0u64)]
    pub instance_id: u64,
    #[default(0i32)]
    pub grid_x: i32,
    #[default(0i32)]
    pub grid_y: i32,
}

/// Normalize a client `/roll` range to the sent `(lo, hi)`: swap an inverted pair (`min > max`)
/// and cap BOTH ends at 10 000, the vanilla client ceiling. `lo <= hi` always holds on the way
/// out. Pure.
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
    fn only_named_creature_broadcast_types_are_supported() {
        assert!(is_supported_chat_type(CHAT_SAY));
        assert!(is_supported_chat_type(CHAT_YELL));
        assert!(is_supported_chat_type(CHAT_TEXT_EMOTE));
        // EMOTE is player-only (`apply_send_chat` accepts it directly); `is_supported_chat_type`
        // serves EventAI, which never emits it.
        assert!(!is_supported_chat_type(CHAT_EMOTE));
        assert!(!is_supported_chat_type(255)); // party/guild/whisper/etc. rejected
    }

    // ---- `apply_send_chat_to`'s type gate (EventAI's only entry into `game_chat_event`) ----
    //
    // `apply_send_chat_to` runs inside a reducer and takes no `ReducerContext` mock in this crate,
    // so its gate is scanned rather than executed, the same technique as the Realm Chat reducers'
    // operator gate scans in `realm_chat.rs`.

    use crate::test_scan::shape_of;

    /// **EventAI's only entry must never admit EMOTE.**
    ///
    /// `apply_send_chat_to`'s `chat_type` comes straight off an imported
    /// `game_creature_ai_broadcast_text` row (`relay.rs`'s `RelayInstruction::Talk`, `engine.rs`'s
    /// `eventai_deliver_line`), and Package import never range-checks that column. Before this fix
    /// the gate read `!is_supported_chat_type(chat_type) && chat_type != CHAT_EMOTE`, so a broadcast
    /// line stamped `chat_type: 3` passed straight through: a `game_chat_event` row with a creature
    /// `sender_guid` and no packet the codec can build for it (`build_chat_message_to` has no
    /// `(EMOTE, Some(name))` arm), silently rendering as Say. EMOTE is a Character's alone, gated by
    /// [`apply_send_chat`]'s own pre-check before this function ever runs.
    ///
    /// Whole-body equality, not a `contains` scan: a second admitting clause appended anywhere in
    /// the function would defeat a substring check but still changes the body this test compares.
    #[test]
    fn apply_send_chat_to_gates_on_is_supported_chat_type_alone() {
        let body = shape_of(include_str!("chat.rs"), "pub(crate) fn apply_send_chat_to(");
        let expected =
            "{ if !is_supported_chat_type(chat_type) { return Err(format!(\"unsupported \
             chat type {chat_type}\")); } write_chat_event(ctx, sender, target_guid, chat_type, \
             language, message) }";
        assert_eq!(
            body, expected,
            "`apply_send_chat_to` no longer gates on `is_supported_chat_type` alone — EventAI's \
             only entry must never admit EMOTE (a Character-only type, gated by `apply_send_chat`), \
             or a Package broadcast line with chat_type 3 renders as Say with no packet the codec \
             knows how to build for a creature."
        );
    }

    /// Wire language values from gtker vanilla `language.rs`: Universal 0, Orcish 1, Common 7.
    #[test]
    fn a_player_line_needs_a_language_the_speakers_race_knows() {
        const HUMAN: u8 = 1;
        const ORC: u8 = 2;
        for chat_type in [CHAT_SAY, CHAT_YELL, CHAT_EMOTE] {
            assert_eq!(
                admit_player_line(chat_type, HUMAN, 1),
                Err("chat:unknown_language".to_string()),
                "a Human in Orcish, chat type {chat_type}"
            );
            assert_eq!(admit_player_line(chat_type, HUMAN, 7), Ok(()));
            assert_eq!(admit_player_line(chat_type, ORC, 1), Ok(()));
            assert_eq!(admit_player_line(chat_type, HUMAN, 0), Ok(()));
        }
    }

    /// The creature-only text emote is EventAI's; a Character never submits it.
    #[test]
    fn a_player_cannot_submit_the_creature_text_emote() {
        assert_eq!(
            admit_player_line(CHAT_TEXT_EMOTE, 1, 0),
            Err("unsupported chat type 2".to_string())
        );
    }

    #[test]
    fn an_addon_payload_keeps_its_whitespace() {
        assert_eq!(
            addon_payload("LCTEST\t ping ").as_deref(),
            Some("LCTEST\t ping ")
        );
        assert_eq!(addon_payload(" "), Some(" ".to_string()));
        assert_eq!(addon_payload(""), None);
        assert_eq!(
            addon_payload(&"a".repeat(1000)).unwrap().chars().count(),
            MAX_CHAT_LEN
        );
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
        let unicode = "🦀".repeat(300);
        assert_eq!(
            normalized_message(&unicode).as_deref(),
            Some("🦀".repeat(MAX_CHAT_LEN).as_str())
        );
    }

    #[test]
    fn an_online_recipient_produces_one_bounded_system_message_event() {
        let recipient_identity = Identity::from_byte_array([7; 32]);
        let created_at = Timestamp::from_micros_since_unix_epoch(42);
        let event = prepare_system_message(
            9001,
            Some((recipient_identity, true)),
            &format!("  {}  ", "🦀".repeat(300)),
            created_at,
        )
        .expect("an online Character accepts one System Message");

        assert_eq!(event.id, 0);
        assert_eq!(event.recipient_identity, recipient_identity);
        assert_eq!(event.recipient_guid, 9001);
        assert_eq!(event.message, "🦀".repeat(MAX_CHAT_LEN));
        assert_eq!(event.created_at, created_at);
    }

    #[test]
    fn a_missing_or_offline_recipient_produces_no_system_message_event() {
        let now = Timestamp::UNIX_EPOCH;
        let identity = Identity::from_byte_array([3; 32]);

        assert_eq!(
            prepare_system_message(9001, None, "hello", now)
                .err()
                .as_deref(),
            Some("system message recipient 9001 is not present on this Shard")
        );
        assert_eq!(
            prepare_system_message(9001, Some((identity, false)), "hello", now)
                .err()
                .as_deref(),
            Some("system message recipient 9001 is offline")
        );
    }

    #[test]
    fn an_empty_system_message_produces_no_event() {
        let identity = Identity::from_byte_array([3; 32]);

        assert_eq!(
            prepare_system_message(9001, Some((identity, true)), "  \t ", Timestamp::UNIX_EPOCH)
                .err()
                .as_deref(),
            Some("empty system message")
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
}
