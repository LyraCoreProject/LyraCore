//! Party/group system: invite → accept/decline handshake, membership, leave/kick/
//! disband, and the group-scoped kill rewards (XP split + shared quest credit).
//!
//! Shape mirrors the friend/ignore system: the GATEWAY resolves typed names to guids
//! (`character_guid_by_name`, the `/who` lookup) and the reducers re-validate server-side off
//! `ctx.sender()`'s in-world character. Cross-player notifications ride a `game_group_event` row
//! (RLS-scoped to the recipient, the `game_whisper_event` pattern) that the gateway relays as
//! `SMSG_GROUP_INVITE` / `SMSG_GROUP_LIST` / `SMSG_GROUP_DECLINE` / `SMSG_GROUP_DESTROYED`; LIST
//! events carry the roster snapshot in their payload, built in the SAME transaction as the
//! membership change (a relay-time coordinator read would race the event across connections).
//!
//! **Where this state LIVES (group slice).** `game_group` / `game_group_member` /
//! `game_group_invite` are authoritative on **realm-core** — none of the three is coupled to space,
//! and all three broke the moment a second database existed (a player inside Deadmines could not
//! invite one in Elwynn: `invite_core` resolved the target inside the CALLING database and a
//! character on another shard has no row there). The gateway drives them there through the
//! operator-gated [`realm_group_op`], and each world shard's copy is a write-through cache it
//! refreshes with [`sync_group_mirror`] — the `game_account`/`game_session` relationship.
//! A single-database deployment has no realm-core to route to and keeps calling the player-facing
//! reducers below exactly as before; the whole split is a gateway routing decision, not a schema one.
//!
//! **Server-driven invites.** A playerbot's serendipity invite has no client and no
//! `ctx.sender()` to resolve — it is a decision the module's own goal tick makes. It cannot write
//! group rows for the same reason a player's cross-shard invite cannot: only the gateway can reach
//! realm-core. [`BotInviteIntent`] is the module's half of that split — a DECISION, not a write —
//! picked up by `gateway/src/world/party.rs`'s `run_bot_invite`. The same row carries a bot's
//! decision to LEAVE its party, which meets the identical authority wall.
//!
//! Vanilla-parity notes for this slice: party cap 5; kill XP splits EVENLY among in-range living
//! members (each member's grey-clamp applies to their OWN level, so a too-high member naturally
//! gets 0) — vanilla's sum-of-levels weighting + 3/4/5-member bonus multipliers are a documented
//! follow-up, not this slice. Kill quest-credit goes to every in-range member (vanilla). Group
//! LOOT methods, round-robin, and raid groups are out of scope.

use spacetimedb::{reducer, table, Identity, ReducerContext, Table, Timestamp};

use crate::{game_character, game_world_entity};

/// Vanilla party size.
pub const GROUP_MAX_MEMBERS: usize = 5;

/// Group kill-reward radius² — members farther than this from the slain creature get neither XP
/// nor quest credit. Vanilla's `sWorld.getConfig(CONFIG_FLOAT_GROUP_XP_DISTANCE)` = 74.0 yd.
pub const GROUP_XP_RANGE_SQ: f32 = 74.0 * 74.0;

/// Loot-method encoding for `Group.loot_method` (work-item 187) — deliberately WIRE-MATCHING: these
/// values are byte-identical to vanilla's `GroupLootSetting` enum (`wow_world_base`:
/// `FreeForAll=0, RoundRobin=1, MasterLoot=2, GroupLoot=3, NeedBeforeGreed=4`), NOT the work item's
/// own draft numbering (which listed `2=GROUP default, 3=MASTER` — the OPPOSITE of the real wire
/// order). Adopting the wire order here means the gateway's `u8 <-> GroupLootSetting` conversion is a
/// straight pass-through with zero translation table — avoiding the exact silent-drift shape an
/// invented enum ordering that doesn't match the wire would cause.
pub mod loot_method {
    pub const FFA: u8 = 0;
    pub const ROUND_ROBIN: u8 = 1;
    pub const MASTER: u8 = 2;
    pub const GROUP: u8 = 3;
    pub const NEED_BEFORE_GREED: u8 = 4;
}

/// A party. `leader_guid` is a member's character guid; leadership transfers on leader-leave and
/// the group disbands below 2 members. Public + no RLS (membership is world-visible state, like
/// `game_world_entity`). NOT gateway-subscribed (verified vs connection.rs, 187 slice 0):
/// SMSG_GROUP_LIST is driven by the module's group event rows, not a direct read of this table —
/// the loot-method columns below ride the SAME roster-payload relay (`roster_payload`), so adding
/// them does NOT require subscribing this table or hand-syncing a gateway binding (verified: no
/// `game_group`/`game_group_member` read anywhere under `gateway/src/` outside the dead,
/// never-subscribed generated binding scaffolding itself).
/// [entity]
#[table(accessor = game_group, public)]
pub struct Group {
    #[primary_key]
    #[auto_inc]
    pub group_id: u64,
    pub leader_guid: u64,
    // END-APPENDED (work-item 187): the party's current loot method/threshold/master + the
    // round-robin cursor. `#[default(...)]` so every pre-187 row keeps behaving as plain FFA-like
    // "no restriction" — wait: the vanilla DEFAULT party loot method is GROUP LOOT (Uncommon
    // threshold), not FFA, so a pre-187 group (created before this slice) auto-migrates to GROUP —
    // matching what a freshly-formed vanilla party actually defaults to. `master_looter_guid` and
    // `rr_cursor` default to 0 regardless of method (meaningless until MASTER is actually selected /
    // the first round-robin kill advances the cursor).
    #[default(3)] // loot_method::GROUP
    pub loot_method: u8,
    #[default(2)] // ItemQuality::Uncommon
    pub loot_threshold: u8,
    #[default(0)]
    pub rr_cursor: u32,
    #[default(0u64)]
    pub master_looter_guid: u64,
}

/// One member row per character in a group. `character_guid` is unique across the table — a
/// character is in at most one group. [entity]
#[table(
    accessor = game_group_member,
    public,
    index(accessor = by_group, btree(columns = [group_id])),
    index(accessor = by_character, btree(columns = [character_guid]))
)]
pub struct GroupMember {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub group_id: u64,
    pub character_guid: u64,
    pub owner_identity: Identity,
}

// Character-owned sweeps: a deleted character leaves its group through the same
// leader-transfer/disband logic a voluntary leave uses — never a bare row delete, which would
// orphan leadership or leave a 1-member group alive.
crate::character_owned!(delete, fn sweep_delete_game_group_member(ctx, character_guid) {
    remove_member(ctx, character_guid);
});
crate::character_owned!(restamp, fn sweep_restamp_game_group_member(ctx, character_guid, identity) {
    let members = ctx.db.game_group_member();
    for mut m in members.by_character().filter(&character_guid).collect::<Vec<_>>() {
        if m.owner_identity != identity {
            m.owner_identity = identity;
            members.id().update(m);
        }
    }
});

// CROSS-DATABASE transport (group slice): membership does NOT ride the export blob.
//
// Shipped an interim MIRROR here — the member row travelled in the manifest carrying its
// original `group_id`, and an `ensure_group` helper re-created the parent `game_group` row at the
// destination. Its own AC#3 called for that mirror to be deleted once membership had one
// authoritative home, and the group slice gives it one: `game_group` / `game_group_member` / `game_group_invite`
// are authoritative on REALM-CORE, and each world shard's copy is a gateway-maintained write-through
// cache (`sync_group_mirror` below) — the same relationship `game_account` / `game_session` have.
//
// So the blob must not carry membership: it would race the authority. The gateway re-pushes the
// realm-core roster onto the destination at world entry (`world::party::on_world_entry`), which is
// strictly better than the snapshot — a party SPLIT across the boundary re-syncs both sides, where
// the snapshot could only carry what the character had when it stepped into the portal.
//
// This also settles the three OPEN entries on the in-transit exception list (`transfer/mod.rs`):
// a third party's accept/kick/leave for an in-transit character is now a REALM-CORE write, so
// there is no source-copy write left to lose.
crate::character_owned!(not_transported, fn sweep_transfer_game_group_member());

/// Drop a character's MIRROR row on the shard it is leaving, without the leave/disband semantics
/// (re-scoped).
///
/// `transfer::do_finish` calls this immediately before `cascade_delete_character` tears the source
/// copy down. A shard hop is not a departure: running `remove_member` would fire DESTROYED at the
/// remaining members and DISBAND a two-person party the moment its first member stepped through the
/// portal. On realm-core that would be worse than wrong — it would be a MIRROR inventing a membership
/// change and notifying clients about it, when the authority (realm-core) recorded nothing at all.
/// Deleting the row and nothing else is exactly what a departing cache entry should do.
///
/// The residue an empty `game_group` row used to leave here is now swept by the gateway's own
/// `sync_group_mirror` push, which replaces this shard's whole copy of the party on the next op or
/// world entry (and deletes the group row outright when the party disbands).
pub(crate) fn detach_for_transfer(ctx: &ReducerContext, character_guid: u64) {
    let members = ctx.db.game_group_member();
    for m in members
        .by_character()
        .filter(&character_guid)
        .collect::<Vec<_>>()
    {
        members.id().delete(m.id);
    }
}

/// A pending invite: at most one per target (a newer invite replaces it). Consumed by
/// accept/decline; a never-answered invite is reaped by the event GC on its own 2-minute TTL
/// (long enough to answer the dialog, short enough not to leak — see `gc.rs`). Private —
/// only the module reads it. [entity]
#[table(accessor = game_group_invite, index(accessor = by_target, btree(columns = [target_guid])))]
pub struct GroupInvite {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub target_guid: u64,
    pub inviter_guid: u64,
    pub created_at: Timestamp,
}

// Character-owned sweep: a deleted character's pending invites go with it — rows where it
// is the TARGET (indexed) and rows where it is the INVITER (scan; the table only ever holds
// currently-pending invites, so it stays tiny). Delete-only: GroupInvite carries no
// owner_identity, so there is nothing to restamp.
crate::character_owned!(delete, fn sweep_delete_game_group_invite(ctx, character_guid) {
    let invites = ctx.db.game_group_invite();
    for inv in invites.by_target().filter(&character_guid).collect::<Vec<_>>() {
        invites.id().delete(inv.id);
    }
    let sent: Vec<u64> = invites
        .iter()
        .filter(|i| i.inviter_guid == character_guid)
        .map(|i| i.id)
        .collect();
    for id in sent {
        invites.id().delete(id);
    }
});
// CROSS-DATABASE transport: a pending invite is a 2-minute dialog with a GC TTL, and
// the inviter is by definition NOT transferring with the target (they are still standing in the
// open world). Carrying it would pop an invite dialog for a party the arriving character cannot see.
// Not transported, by decision — the invite dies with the source copy, exactly as a decline would.
crate::character_owned!(not_transported, fn sweep_transfer_game_group_invite());

/// A bot-initiated invite the MODULE has DECIDED but cannot execute (closing the gap
/// the group slice opened): `game_group`/`game_group_member` are authoritative on realm-core, and
/// only the gateway can reach it — the module never has. Before this table existed, playerbots'
/// serendipity invite (`packages/playerbots/src/goals.rs`, `maybe_invite_fellow_quester`) called
/// [`invite_core`] directly, writing this shard's LOCAL `game_group`/`game_group_member` rows, which
/// realm-core had never heard of; the next `sync_group_mirror` push then wiped the party the mirror
/// did not recognise.
///
/// The fix splits the decision from the execution, the same way `world::party` already splits a
/// player's own invite: the module picks the fellow quester (it already owns the spatial + quest
/// data that decision needs) and writes it HERE instead of writing group rows; the gateway's
/// `world::party::run_bot_invite` picks the row up and runs the SAME `realm_group_op` a player's own
/// CMSG_GROUP_INVITE would, so a bot party is created through the authority a `sync_group_mirror`
/// push never contradicts.
///
/// The row carries a LEAVE as well as an INVITE now (see [`op`](Self::op)) — a bot leaving a party
/// hits the same authority wall the invite did. The table keeps its name because renaming one is a
/// migration and this is not.
///
/// Private — no client ever needs to see this, only the gateway's owner-token coordinator connection
/// (the `game_account`/`game_session` pattern). Short-lived: reaped by
/// the shared 1s event TTL (`gc.rs`), which is generous — the gateway's subscription callback fires
/// on the insert, not on a poll. [entity]
#[table(accessor = game_bot_invite_intent)]
pub struct BotInviteIntent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub inviter_guid: u64,
    pub target_guid: u64,
    pub created_at: Timestamp,
    /// Which group op the gateway should run: [`lyracore_shared::group::bot_op`]. END-appended with
    /// a `0` default, so every row written before this column existed reads as the INVITE it was.
    ///
    /// A leave needs the same relay for the same reason an invite does — membership is
    /// authoritative on realm-core and the module cannot reach it — so it rides this row rather
    /// than a second table with an identical shape and an identical reaper.
    #[default(0u8)]
    pub op: u8,
}

/// Record a bot's serendipity invite DECISION for the gateway to execute. No gating here
/// beyond existence-of-nothing — every real gate (leader-only, party cap, already-grouped,
/// pending-invite-replaces-older) lives in `invite_core_on`/`realm_group_op`, which the gateway calls
/// against the correct authority; this is a pure write, mirroring how a player's own CMSG_GROUP_INVITE
/// is a pure gateway-side resolve-then-call with no module-side pre-check either.
///
/// Its ONLY caller is the playerbots drop-in (see the `package_only!` macro in `actor.rs`): a
/// build with no REAL package installed — the common case, since only the inert reference Package,
/// `packages/example/`, ships by default — has no caller for this, which is a designed state, not
/// dead code.
#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn emit_bot_invite_intent(ctx: &ReducerContext, inviter_guid: u64, target_guid: u64) {
    emit_bot_group_intent(ctx, bot_op::INVITE, inviter_guid, target_guid);
}

/// Record a session-less Character's decision to LEAVE its party, for the gateway to execute.
///
/// The same split, and the same reason: a bot that ran `leave_group_for` itself would write this
/// shard's local member rows, and the next `sync_group_mirror` push would put the party back.
///
/// Nothing here checks that the Character is in a party. `leave_group_for` refuses a non-member, so
/// a decision that has already been executed costs one refused op rather than a wrong one.
#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn emit_bot_leave_intent(ctx: &ReducerContext, leaver_guid: u64) {
    emit_bot_group_intent(ctx, bot_op::LEAVE, leaver_guid, 0);
}

/// The one writer of [`BotInviteIntent`]. `target_guid` is unused by a LEAVE.
#[cfg_attr(not(has_packages), allow(dead_code))]
fn emit_bot_group_intent(ctx: &ReducerContext, op: u8, actor_guid: u64, target_guid: u64) {
    ctx.db.game_bot_invite_intent().insert(BotInviteIntent {
        id: 0,
        inviter_guid: actor_guid,
        target_guid,
        created_at: ctx.timestamp,
        op,
    });
}

/// Atomically remove one bot invite intent before a Gateway executes it.
///
/// Every Gateway subscribes to the same World Shard row. SpacetimeDB serializes reducer
/// transactions, so the direct primary-key delete admits one caller and refuses every later
/// callback, including callbacks installed after a watchdog reconnect.
#[reducer]
pub fn claim_bot_invite_intent(ctx: &ReducerContext, intent_id: u64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if !ctx.db.game_bot_invite_intent().id().delete(intent_id) {
        return Err(refused(
            GroupRefusal::IntentAlreadyClaimed,
            &format!("bot invite intent {intent_id} is gone"),
        ));
    }
    Ok(())
}

/// Reducer edge: only the tag crosses to the gateway; the detail stays in module logs.
pub(crate) fn refused(refusal: GroupRefusal, detail: &str) -> String {
    let tag = refusal.as_tag();
    spacetimedb::log::info!("group refused {tag}: {detail}");
    tag.to_string()
}

// Event kinds, roster grammar, and classified error strings are the SHARED wire contract:
// lyracore_shared::group is the one definition both crates import — a renumber,
// reword, or delimiter change is a cross-crate compile-visible edit, never a runtime drift.
use lyracore_shared::group::{bot_op, event_kind as group_event_kind, GroupRefusal};

/// A per-recipient group notification (the `game_whisper_event` pattern): public + RLS-scoped so
/// only the recipient's connection sees it; reaped by the shared event GC. `other_name` is
/// resolved at write time so the gateway never needs a name lookup for INVITE/DECLINE. LIST events
/// carry the FULL roster snapshot in `payload` ("leader|guid,name,online;...") — built in the SAME
/// transaction as the membership change, so the gateway relay never races a cross-connection
/// coordinator read (the module is the one place the roster is guaranteed consistent). [event]
#[table(accessor = game_group_event, public, index(accessor = by_recipient, btree(columns = [recipient_identity])))]
pub struct GroupEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub recipient_identity: Identity,
    pub kind: u8, // group_event_kind::*
    pub other_guid: u64,
    pub other_name: String,
    pub created_at: Timestamp,
    // LIST roster snapshot ("leader|guid,name,online;..."); empty for the other kinds. A plain
    // column (String cannot be #[default]-ed — the macro's typecheck is const): fine because this
    // whole table has no pre-payload row to migrate anywhere real.
    pub payload: String,
    /// The recipient's CHARACTER GUID (group slice). END-appended + `#[default(0)]`, so
    /// this is an additive auto-migration and every earlier row reads as 0.
    ///
    /// `recipient_identity` cannot address a recipient on REALM-CORE: an identity is minted per
    /// (account, database) by the node, so the identity a player holds on a world shard names
    /// nobody on the directory database — and realm-core has no `game_character` rows to resolve one
    /// from in the first place. A guid is the one realm-wide name a character has. The identity
    /// column stays exactly as it was and still drives the per-player RLS on a world shard; this
    /// column is what the gateway's realm-core relay filters on (it reads through the owner token,
    /// which bypasses RLS, and self-filters per session — the coordinator-relay law of 277/279).
    // The u64 default MUST be typed: a bare `0` encodes as 4 bytes and `publish` rejects the
    // migration with "data too short for u64: Expected 8, given 4" (world.rs:127 records the same
    // rule). Nothing in `cargo test`/`cargo check` validates default-value encoding — only a real
    // migration does — so this shipped green and blocked the first publish that tried to apply it.
    #[default(0u64)]
    pub recipient_guid: u64,
}

pub(crate) fn push_event(
    ctx: &ReducerContext,
    recipient_guid: u64,
    kind: u8,
    other_guid: u64,
    payload: String,
) {
    // No longer an early return on a missing character row. On a world shard the row is there
    // and this is byte-identical to before; on REALM-CORE there are no character rows at all, and
    // returning early there would mean the directory database could never notify anybody — every
    // invite popup and roster refresh for a cross-shard party would be dropped at the source.
    let bound = ctx
        .db
        .game_character()
        .guid()
        .find(recipient_guid)
        .map(|c| c.owner_identity);
    let other_name = ctx
        .db
        .game_character()
        .guid()
        .find(other_guid)
        .map(|c| c.name)
        .unwrap_or_default();
    ctx.db.game_group_event().insert(GroupEvent {
        id: 0,
        recipient_identity: crate::helpers::event_recipient_identity(bound),
        kind,
        other_guid,
        other_name,
        created_at: ctx.timestamp,
        payload,
        recipient_guid,
    });
}

/// The LIST payload rows, encoded by the SHARED grammar (`lyracore_shared::group::encode_roster` —
/// delimiter defense included). "online" = a live world entity exists (a session-less playerbot
/// counts). Carries the group's CURRENT loot method/threshold/master (work-item 187) alongside the
/// roster, so a `CMSG_LOOT_METHOD` change re-renders the party frame's loot block through this same
/// relay — no separate gateway round trip needed.
/// `None` means the `game_group` row is MISSING — every caller treats that as a hard invariant
/// violation (they've either just inserted the row or already `.ok_or("group row missing")?`'d
/// their own read of it), so there is no plausible roster to fabricate here. Previously this
/// synthesized a fake one (leader 0, GROUP method, threshold 2, no master) that looked like a
/// real, empty party.
fn roster_payload(ctx: &ReducerContext, group_id: u64) -> Option<String> {
    let group = ctx.db.game_group().group_id().find(group_id)?;
    let members: Vec<(u64, String, bool)> = members_of(ctx, group_id)
        .into_iter()
        .map(|m| {
            let name = ctx
                .db
                .game_character()
                .guid()
                .find(m.character_guid)
                .map(|c| c.name)
                .unwrap_or_default();
            let online = ctx
                .db
                .game_world_entity()
                .guid()
                .find(m.character_guid)
                .is_some();
            (m.character_guid, name, online)
        })
        .collect();
    Some(lyracore_shared::group::encode_roster(
        group.leader_guid,
        group.loot_method,
        group.loot_threshold,
        group.master_looter_guid,
        &members,
    ))
}

/// The group a character belongs to, if any.
pub(crate) fn group_of(ctx: &ReducerContext, character_guid: u64) -> Option<GroupMember> {
    ctx.db
        .game_group_member()
        .by_character()
        .filter(&character_guid)
        .next()
}

/// The leader-authorization sequence shared by `invite_core_on` / `uninvite_on` /
/// `set_loot_method_on`: resolve `guid`'s group membership, its `Group` row, and confirm
/// `guid` actually IS that group's leader. [`GroupRefusal::NotInGroup`] if `guid` has no group at
/// all; [`GroupRefusal::NotLeader`] if it does but isn't the leader. `invite_core_on` treats
/// `NotInGroup` as an ALLOWED case rather than propagating it — starting a brand-new group (where
/// the inviter becomes leader) is fine; see its call site.
pub(crate) fn led_group_of(
    ctx: &ReducerContext,
    guid: u64,
) -> Result<(GroupMember, Group), GroupRefusal> {
    let m = group_of(ctx, guid).ok_or(GroupRefusal::NotInGroup)?;
    let group = ctx
        .db
        .game_group()
        .group_id()
        .find(m.group_id)
        .ok_or(GroupRefusal::Database)?;
    if group.leader_guid != guid {
        return Err(GroupRefusal::NotLeader);
    }
    Ok((m, group))
}

/// `pub(crate)` (was private): work-item 187's kill-time loot stamping (`combat/mod.rs` →
/// `loot::apply_group_loot_rules`) needs the CURRENT roster to pick a round-robin/master designee.
pub(crate) fn members_of(ctx: &ReducerContext, group_id: u64) -> Vec<GroupMember> {
    ctx.db
        .game_group_member()
        .by_group()
        .filter(&group_id)
        .collect()
}

fn push_list_to_all(ctx: &ReducerContext, group_id: u64) {
    let Some(payload) = roster_payload(ctx, group_id) else {
        spacetimedb::log::warn!(
            "push_list_to_all: group {group_id} row missing — skipping roster push (broken invariant, not a fabricated empty roster)"
        );
        return;
    };
    for m in members_of(ctx, group_id) {
        push_event(
            ctx,
            m.character_guid,
            group_event_kind::LIST,
            0,
            payload.clone(),
        );
    }
}

// ===========================================================================================
//  Reducers (sender-identity, gateway-resolved guids — the add_friend convention)
// ===========================================================================================

/// `CMSG_GROUP_INVITE` (name gateway-resolved to `target_guid`): validate, record the pending
/// invite (replacing any older one on the target), notify the target, and fire the
/// `on_group_invite` package hook (a bot target auto-accepts through it — but only on the plane where
/// the bot's own rows live: see [`invite_core_on`]'s note on the hook). Reached via
/// `gw::gw_group_invite`.
///
/// The identity-free invite core (the `accept_invite_for` pattern): shared by `gw::gw_group_invite`
/// and any server-driven inviter — a playerbot's serendipity invite (276) calls this with
/// the bot's guid. Same gates in the same order for every caller.
pub(crate) fn invite_core(
    ctx: &ReducerContext,
    inviter_guid: u64,
    target_guid: u64,
) -> Result<(), String> {
    invite_core_on(ctx, Plane::Shard, inviter_guid, target_guid)
        .map_err(|r| refused(r, &format!("{inviter_guid} could not invite {target_guid}")))
}

fn invite_core_on(
    ctx: &ReducerContext,
    plane: Plane,
    inviter_guid: u64,
    target_guid: u64,
) -> Result<(), GroupRefusal> {
    if target_guid == inviter_guid {
        return Err(GroupRefusal::InviteSelf);
    }
    // EXISTENCE + PRESENCE are the two gates that need a database holding characters and live
    // entities, so they are the two the directory plane cannot run: realm-core has
    // neither table populated, and a shard's copy only knows about its own players — which is the
    // whole bug this slice fixes (a target inside Deadmines "does not exist" to the open world).
    // On REALM-CORE the gateway has already resolved both ACROSS every connected shard before
    // calling (`world::party::resolve_target`), and it answers with the same two Refusals, so the
    // player-visible answer is unchanged.
    if plane == Plane::Shard {
        if ctx.db.game_character().guid().find(target_guid).is_none() {
            return Err(GroupRefusal::NoSuchPlayer);
        }
        // Vanilla requires the target online; a session-less playerbot's live entity counts (its
        // auto-accept rides the hook below, not a client).
        if ctx
            .db
            .game_world_entity()
            .guid()
            .find(target_guid)
            .is_none()
        {
            return Err(GroupRefusal::TargetOffline);
        }
    }
    if group_of(ctx, target_guid).is_some() {
        return Err(GroupRefusal::AlreadyInGroup);
    }
    // The inviter having NO group yet is fine (they'll lead a brand-new one) — only propagate
    // NotLeader (inviter is in a group but isn't its leader); a led group additionally enforces the
    // member cap.
    match led_group_of(ctx, inviter_guid) {
        Ok((m, _group)) => {
            if members_of(ctx, m.group_id).len() >= GROUP_MAX_MEMBERS {
                return Err(GroupRefusal::GroupFull);
            }
        }
        Err(GroupRefusal::NotInGroup) => {}
        Err(refusal) => return Err(refusal),
    }
    let invites = ctx.db.game_group_invite();
    for stale in invites.by_target().filter(&target_guid).collect::<Vec<_>>() {
        invites.id().delete(stale.id);
    }
    invites.insert(GroupInvite {
        id: 0,
        target_guid,
        inviter_guid,
        created_at: ctx.timestamp,
    });
    push_event(
        ctx,
        target_guid,
        group_event_kind::INVITE,
        inviter_guid,
        String::new(),
    );
    // The hook is PLANE-LOCAL, and this is what that costs: package tables live on the world
    // shards, so on a TRUE multi-database realm-core `pkg_playerbots_bot` is empty and the playerbots
    // auto-accept handler returns immediately — a player's invite to a bot was created correctly and
    // then nobody answered it (observed live 2026-07-26). Fired here regardless: a single-database
    // gateway's realm-core IS the world shard, so `pkg_playerbots_bot` is populated and the hook
    // still answers in this transaction there — including every bot-to-bot serendipity invite
    // which since that fix runs through `realm_group_op`/Plane::RealmCore like every
    // other invite, never through `invite_core`/Plane::Shard directly. On a real multi-database
    // deployment the gateway answers instead, where it can see both databases
    // (`gateway/src/world/party.rs`, `answer_for_session_less`).
    crate::hooks::fire_on_group_invite(
        ctx,
        &crate::hooks::GroupInvitePayload {
            target_guid,
            inviter_guid,
        },
    );
    Ok(())
}

/// `CMSG_GROUP_ACCEPT`: consume the caller's pending invite; create the inviter's group if this
/// is its first acceptance, join it otherwise, and push a roster refresh to every member. Reached
/// via `gw::gw_accept_group_invite`.
///
/// The identity-free accept core: shared by `gw::gw_accept_group_invite` and any server-driven
/// acceptor (a playerbot's auto-accept hook calls this with the bot's guid).
pub(crate) fn accept_invite_for(ctx: &ReducerContext, acceptor_guid: u64) -> Result<(), String> {
    accept_invite_on(ctx, Plane::Shard, acceptor_guid)
        .map_err(|r| refused(r, &format!("{acceptor_guid} could not accept its invite")))
}

fn accept_invite_on(
    ctx: &ReducerContext,
    plane: Plane,
    acceptor_guid: u64,
) -> Result<(), GroupRefusal> {
    let invites = ctx.db.game_group_invite();
    let invite = invites
        .by_target()
        .filter(&acceptor_guid)
        .next()
        .ok_or(GroupRefusal::NoPendingInvite)?;
    invites.id().delete(invite.id);
    if group_of(ctx, acceptor_guid).is_some() {
        return Err(GroupRefusal::AlreadyInGroup);
    }
    let inviter_guid = invite.inviter_guid;
    // The member row's `owner_identity` is the SHARD's binding for that character. On realm-core
    // there is no such binding to read (identities are per-database), so the directory plane stores
    // ZERO and each shard's mirror re-derives its own — which is the same thing `player_login`'s
    // restamp does for every other character-owned row. The "inviter no longer exists" gate is
    // therefore a SHARD-plane gate; on realm-core the inviter's continued existence is proven by the
    // group/membership rows the branches below read, not by a character row that was never there.
    let inviter_identity = match plane {
        Plane::Shard => Some(
            ctx.db
                .game_character()
                .guid()
                .find(inviter_guid)
                .ok_or(GroupRefusal::InviterUnavailable)?
                .owner_identity,
        ),
        Plane::RealmCore => None,
    };
    let members = ctx.db.game_group_member();
    let group_id = match group_of(ctx, inviter_guid) {
        Some(m) => {
            // Re-run the invite-time leadership gate: the invite was issued when the inviter was
            // the leader (or ungrouped and about to lead). If they since joined a DIFFERENT group
            // as a plain member, honoring the stale invite would smuggle the acceptor into a group
            // whose leader never invited them.
            let group = ctx
                .db
                .game_group()
                .group_id()
                .find(m.group_id)
                .ok_or(GroupRefusal::Database)?;
            if group.leader_guid != inviter_guid {
                return Err(GroupRefusal::InviterUnavailable);
            }
            if members_of(ctx, m.group_id).len() >= GROUP_MAX_MEMBERS {
                return Err(GroupRefusal::GroupFull);
            }
            m.group_id
        }
        None => {
            // First acceptance forms the group: the inviter leads and joins it here.
            let group = ctx.db.game_group().insert(Group {
                group_id: 0,
                leader_guid: inviter_guid,
                // Vanilla's real default for a freshly-formed party (work-item 187): GROUP LOOT at
                // Uncommon threshold, no master, cursor at 0.
                loot_method: loot_method::GROUP,
                loot_threshold: 2,
                rr_cursor: 0,
                master_looter_guid: 0,
            });
            members.insert(GroupMember {
                id: 0,
                group_id: group.group_id,
                character_guid: inviter_guid,
                owner_identity: crate::helpers::event_recipient_identity(inviter_identity),
            });
            group.group_id
        }
    };
    let acceptor_identity = match plane {
        Plane::Shard => ctx
            .db
            .game_character()
            .guid()
            .find(acceptor_guid)
            .map(|c| c.owner_identity)
            .ok_or(GroupRefusal::NoSuchPlayer)?,
        Plane::RealmCore => Identity::ZERO,
    };
    members.insert(GroupMember {
        id: 0,
        group_id,
        character_guid: acceptor_guid,
        owner_identity: acceptor_identity,
    });
    push_list_to_all(ctx, group_id);
    Ok(())
}

/// The identity-free decline core: the body `group_decline` used to inline, so the realm-core
/// plane runs the SAME code rather than a second implementation of it.
pub(crate) fn decline_invite_for(ctx: &ReducerContext, decliner_guid: u64) -> Result<(), String> {
    decline_invite_on(ctx, decliner_guid)
        .map_err(|r| refused(r, &format!("{decliner_guid} could not decline an invite")))
}

fn decline_invite_on(ctx: &ReducerContext, decliner_guid: u64) -> Result<(), GroupRefusal> {
    let invites = ctx.db.game_group_invite();
    let invite = invites
        .by_target()
        .filter(&decliner_guid)
        .next()
        .ok_or(GroupRefusal::NoPendingInvite)?;
    invites.id().delete(invite.id);
    push_event(
        ctx,
        invite.inviter_guid,
        group_event_kind::DECLINE,
        decliner_guid,
        String::new(),
    );
    Ok(())
}

/// The identity-free leave core — the body `group_leave` used to inline.
pub(crate) fn leave_group_for(ctx: &ReducerContext, leaver_guid: u64) -> Result<(), String> {
    leave_group_on(ctx, leaver_guid)
        .map_err(|r| refused(r, &format!("{leaver_guid} could not leave its party")))
}

fn leave_group_on(ctx: &ReducerContext, leaver_guid: u64) -> Result<(), GroupRefusal> {
    if group_of(ctx, leaver_guid).is_none() {
        return Err(GroupRefusal::NotInGroup);
    }
    remove_member(ctx, leaver_guid);
    Ok(())
}

/// The identity-free kick core — the body `group_uninvite` used to inline.
pub(crate) fn uninvite_from_group(
    ctx: &ReducerContext,
    leader_guid: u64,
    target_guid: u64,
) -> Result<(), String> {
    uninvite_on(ctx, leader_guid, target_guid)
        .map_err(|r| refused(r, &format!("{leader_guid} could not kick {target_guid}")))
}

fn uninvite_on(
    ctx: &ReducerContext,
    leader_guid: u64,
    target_guid: u64,
) -> Result<(), GroupRefusal> {
    let (m, _group) = led_group_of(ctx, leader_guid)?;
    let target = group_of(ctx, target_guid).ok_or(GroupRefusal::TargetNotInGroup)?;
    if target.group_id != m.group_id {
        return Err(GroupRefusal::TargetNotInGroup);
    }
    if target_guid == leader_guid {
        return Err(GroupRefusal::KickSelf);
    }
    remove_member(ctx, target_guid);
    Ok(())
}

/// The identity-free loot-method core — the body `group_loot_method` used to inline.
pub(crate) fn set_loot_method_for(
    ctx: &ReducerContext,
    leader_guid: u64,
    loot_setting: u8,
    master_guid: u64,
    loot_threshold: u8,
) -> Result<(), String> {
    set_loot_method_on(ctx, leader_guid, loot_setting, master_guid, loot_threshold).map_err(|r| {
        refused(
            r,
            &format!("{leader_guid} could not set the party loot rules"),
        )
    })
}

fn set_loot_method_on(
    ctx: &ReducerContext,
    leader_guid: u64,
    loot_setting: u8,
    master_guid: u64,
    loot_threshold: u8,
) -> Result<(), GroupRefusal> {
    let (m, mut group) = led_group_of(ctx, leader_guid)?;
    if !valid_loot_method(loot_setting) || !valid_loot_threshold(loot_threshold) {
        return Err(GroupRefusal::InvalidLootRules);
    }
    let resolved_master = if loot_setting == loot_method::MASTER {
        let target = group_of(ctx, master_guid).ok_or(GroupRefusal::TargetNotInGroup)?;
        if target.group_id != m.group_id {
            return Err(GroupRefusal::TargetNotInGroup);
        }
        master_guid
    } else {
        0 // meaningless for a non-MASTER method — never leave a stale master guid set
    };
    group.loot_method = loot_setting;
    group.loot_threshold = loot_threshold;
    group.master_looter_guid = resolved_master;
    ctx.db.game_group().group_id().update(group);
    push_list_to_all(ctx, m.group_id);
    Ok(())
}

/// Is `method` one of the five known `loot_method::*` values? Pure. [`group_loot_method`]'s gate,
/// split out so it's unit-testable without a live `ReducerContext`.
pub(crate) fn valid_loot_method(method: u8) -> bool {
    matches!(
        method,
        loot_method::FFA
            | loot_method::ROUND_ROBIN
            | loot_method::MASTER
            | loot_method::GROUP
            | loot_method::NEED_BEFORE_GREED
    )
}

/// Is `threshold` a real vanilla `ItemQuality` (0 Poor..=6 Artifact)? Pure.
pub(crate) fn valid_loot_threshold(threshold: u8) -> bool {
    threshold <= 6
}

/// The single membership-removal core (voluntary leave, kick, character delete): drops the member
/// row, notifies the leaver, transfers leadership if the leader left, and DISBANDS below 2 members
/// (vanilla: a party of one is no party). Idempotent for a guid not in any group.
pub(crate) fn remove_member(ctx: &ReducerContext, character_guid: u64) {
    let Some(m) = group_of(ctx, character_guid) else {
        return;
    };
    let group_id = m.group_id;
    crate::loot::tag::revoke_group_member(ctx, group_id, character_guid);
    ctx.db.game_group_member().id().delete(m.id);
    push_event(
        ctx,
        character_guid,
        group_event_kind::DESTROYED,
        0,
        String::new(),
    );

    let remaining = members_of(ctx, group_id);
    let pairs: Vec<(u64, u64)> = remaining.iter().map(|r| (r.id, r.character_guid)).collect();
    let group = ctx.db.game_group().group_id().find(group_id);
    // The incumbent-leader arg only matters for the survive branch; a missing group row (shouldn't
    // happen while members exist) falls back to 0 — no update runs without a row to update anyway.
    match leader_after_removal(
        &pairs,
        character_guid,
        group.as_ref().map_or(0, |g| g.leader_guid),
    ) {
        None => {
            // Work-item 187 trap: a group disbanding mid-roll must resolve any of its live rolls
            // to the sole remaining member (vanilla behavior) rather than leaving them to time out.
            // The FULL former membership (the leaver, already removed above, PLUS whoever's left) is
            // what `force_resolve_rolls_for_disband` needs to recognize "every voter on this roll
            // belonged to the group that just disbanded" — `remaining` alone would miss the leaver.
            let survivors: Vec<u64> = remaining.iter().map(|r| r.character_guid).collect();
            let mut all_guids = survivors.clone();
            all_guids.push(character_guid);
            let survivor = if survivors.len() == 1 {
                Some(survivors[0])
            } else {
                None
            };
            crate::loot::force_resolve_rolls_for_disband(ctx, &all_guids, survivor);
            for r in &remaining {
                push_event(
                    ctx,
                    r.character_guid,
                    group_event_kind::DESTROYED,
                    0,
                    String::new(),
                );
                ctx.db.game_group_member().id().delete(r.id);
            }
            ctx.db.game_group().group_id().delete(group_id);
            return;
        }
        Some(new_leader) => {
            if let Some(mut group) = group {
                if group.leader_guid != new_leader {
                    group.leader_guid = new_leader;
                    ctx.db.game_group().group_id().update(group);
                }
            }
        }
    }
    push_list_to_all(ctx, group_id);
}

/// The post-removal survival/leadership decision for [`remove_member`]: `None` → the group DISBANDS
/// (fewer than 2 members remain — vanilla: a party of one is no party); `Some(leader)` → the group
/// survives under `leader`, the incumbent unless the leaver WAS the leader, in which case leadership
/// passes to the longest-standing remaining member (lowest member-row id). `remaining` is the
/// `(member_row_id, character_guid)` pairs left AFTER the leaver's row is deleted. Pure — unit-tested.
pub(crate) fn leader_after_removal(
    remaining: &[(u64, u64)],
    leaving_guid: u64,
    leader_guid: u64,
) -> Option<u64> {
    if remaining.len() < 2 {
        return None;
    }
    if leader_guid == leaving_guid {
        // Leadership passes to the longest-standing remaining member (lowest row id).
        let heir = remaining
            .iter()
            .min_by_key(|(id, _)| *id)
            .expect("len >= 2");
        Some(heir.1)
    } else {
        Some(leader_guid)
    }
}

// ===========================================================================================
//  REALM-CORE plane (group slice)
// ===========================================================================================

/// Which DEPLOYMENT of this one module a group call is running on.
///
/// Realm-core is not a different crate — it is this same wasm published under a second database name
/// (`realm_core.rs`'s header). Every table exists on both; what differs is which rows are ever
/// written there. On a WORLD shard `game_character` and `game_world_entity` are populated, so the
/// existence/presence gates read them. On REALM-CORE neither is (the directory database holds
/// accounts, sessions, the character→shard index and — from this slice — party membership), so those
/// two gates have no data to run against and the GATEWAY supplies their answers instead, resolved
/// across every connected shard. Nothing else about the rules changes, which is the point: the same
/// three cores run on both planes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Plane {
    /// A world shard: characters and live entities are local.
    Shard,
    /// The realm-wide directory database: neither is.
    RealmCore,
}

/// The realm-wide party ops, as ONE operator-gated reducer keyed by [`lyracore_shared::group::realm_op`].
///
/// **Operator-gated, and it has to be**: it takes the acting character's guid as an argument instead
/// of deriving it from `ctx.sender()`, because on realm-core there is no live entity to derive it
/// from. A client that could call it could accept invites, kick members and change loot rules as
/// anybody in the realm. The gateway is the only caller, it holds the coordinator (operator) token,
/// and it passes the guid it already authenticated for that socket (`InWorld::self_guid`) — the same
/// trust boundary `set_character_shard` and `establish_session` sit on.
///
/// One reducer rather than six: see [`lyracore_shared::group::realm_op`] for that trade and for the
/// argument slots each op reads.
#[reducer]
pub fn realm_group_op(
    ctx: &ReducerContext,
    op: u8,
    actor_guid: u64,
    target_guid: u64,
    arg_a: u8,
    arg_b: u8,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    use lyracore_shared::group::realm_op;
    // An op byte this module does not know is a gateway newer than the module — a deployment fault,
    // not a party outcome, so it stays an untagged error the gateway treats as a failure.
    let ran = match op {
        realm_op::INVITE => invite_core_on(ctx, Plane::RealmCore, actor_guid, target_guid),
        realm_op::ACCEPT => accept_invite_on(ctx, Plane::RealmCore, actor_guid),
        realm_op::DECLINE => decline_invite_on(ctx, actor_guid),
        realm_op::LEAVE => leave_group_on(ctx, actor_guid),
        realm_op::UNINVITE => uninvite_on(ctx, actor_guid, target_guid),
        // `CMSG_LOOT_METHOD`'s own field order: setting, master, threshold.
        realm_op::LOOT_METHOD => set_loot_method_on(ctx, actor_guid, arg_a, target_guid, arg_b),
        other => return Err(format!("unknown realm group op {other}")),
    };
    ran.map_err(|r| refused(r, &format!("realm group op {op} for {actor_guid}")))
}

/// Replace this database's MIRROR of one party with realm-core's authoritative roster.
///
/// The world shards keep their `game_group` / `game_group_member` tables — the constraint is that a
/// live deployment cannot drop a table, and the invalidation story is the better answer anyway:
/// roughly fifty in-world reads (kill-XP split and quest credit in `combat`, the loot-method and
/// round-robin/master rules in `loot.rs`, `/p` chat in `chat.rs`, the party's dungeon binding in
/// `instance.rs`) resolve membership through `group_of`/`members_of` against the LOCAL rows, and
/// every one of them is a hot-path read that must not become a cross-database call. So the shard's
/// copy becomes a gateway-maintained write-through cache of realm-core's, exactly as `game_account`
/// and `game_session` became caches of realm-core's copies.
///
/// Invalidation: the gateway re-pushes this after every party op it runs (to every connected world
/// shard) and at every world entry (to the shard the character just entered — which is what carries
/// a party across a shard boundary now that the blob mirror is gone). A shard that misses a push
/// re-syncs on the next of either. Nothing here NOTIFIES anybody: the realm-core plane owns the
/// client-facing relay, and a mirror that pushed its own `SMSG_GROUP_LIST` would double-render the
/// party frame on the acting player's shard.
///
/// Mechanical by design — no leadership arbitration, no disband rule, no roll resolution. Those are
/// decisions, and decisions belong to the authority. An empty `members` is the disband/last-member
/// case and deletes the group row.
#[reducer]
pub fn sync_group_mirror(
    ctx: &ReducerContext,
    group_id: u64,
    leader_guid: u64,
    loot_method_setting: u8,
    loot_threshold: u8,
    master_looter_guid: u64,
    members: Vec<u64>,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if group_id == 0 {
        return Err("group 0 is not a party".to_string());
    }
    let groups = ctx.db.game_group();
    let member_tbl = ctx.db.game_group_member();
    let current: Vec<(u64, u64)> = member_tbl
        .by_group()
        .filter(&group_id)
        .map(|m| (m.id, m.character_guid))
        .collect();
    let (stale_row_ids, arriving) = mirror_plan(&current, &members);
    for id in stale_row_ids {
        if let Some((_, guid)) = current.iter().find(|(row_id, _)| *row_id == id) {
            crate::loot::tag::revoke_group_member(ctx, group_id, *guid);
        }
        member_tbl.id().delete(id);
    }
    if members.is_empty() {
        groups.group_id().delete(group_id);
        return Ok(());
    }
    // A character is in at most one group (the `by_character` uniqueness the whole system assumes),
    // so an arriving member's row in some OTHER group on this shard — a mirror that missed the push
    // where they left it — is stale by construction and goes first.
    for guid in &arriving {
        for m in member_tbl.by_character().filter(guid).collect::<Vec<_>>() {
            crate::loot::tag::revoke_group_member(ctx, m.group_id, *guid);
            member_tbl.id().delete(m.id);
        }
    }
    match groups.group_id().find(group_id) {
        Some(mut g) => {
            g.leader_guid = leader_guid;
            g.loot_method = loot_method_setting;
            g.loot_threshold = loot_threshold;
            g.master_looter_guid = master_looter_guid;
            groups.group_id().update(g);
        }
        None => {
            groups.insert(Group {
                group_id,
                leader_guid,
                loot_method: loot_method_setting,
                loot_threshold,
                // The round-robin cursor is per-SHARD kill state, not realm state: it advances on
                // this shard's kills and means nothing on another. A fresh mirror starts it at 0.
                rr_cursor: 0,
                master_looter_guid,
            });
        }
    }
    for guid in arriving {
        // The identity binding is this shard's, re-derived here the same way `player_login` restamps
        // every other character-owned row. A member who has never logged in HERE — or who is mid-hop,
        // which is why this reads through the in-transit chokepoint rather than raw — has no
        // readable character row and mirrors as ZERO; their own login restamps it.
        let owner_identity = crate::helpers::event_recipient_identity(
            crate::helpers::character_by_guid(ctx, guid).map(|c| c.owner_identity),
        );
        member_tbl.insert(GroupMember {
            id: 0,
            group_id,
            character_guid: guid,
            owner_identity,
        });
    }
    Ok(())
}

/// The row-level diff [`sync_group_mirror`] applies: which mirrored member ROWS of this group are no
/// longer in the authoritative roster (delete them, by row id) and which roster members have no row
/// yet (insert them, by guid). Pure — the reducer around it is unreachable without a live node, so
/// the decision it makes is extracted here where a test can drive it.
///
/// `current` is `(member_row_id, character_guid)` for this group's rows on THIS database.
pub(crate) fn mirror_plan(current: &[(u64, u64)], incoming: &[u64]) -> (Vec<u64>, Vec<u64>) {
    let stale = current
        .iter()
        .filter(|(_, guid)| !incoming.contains(guid))
        .map(|(id, _)| *id)
        .collect();
    let arriving = incoming
        .iter()
        .filter(|guid| !current.iter().any(|(_, have)| have == *guid))
        .copied()
        .collect();
    (stale, arriving)
}

// ===========================================================================================
//  Kill rewards: XP split + shared quest credit
// ===========================================================================================

/// Everyone a kill by `killer_guid` at `(x, y)` on `map_id`/`instance_id` (the kill site) rewards:
/// the killer alone when ungrouped, else every group member whose LIVE entity is alive, on the
/// same map + instance, and within [`GROUP_XP_RANGE_SQ`] of the corpse (the killer qualifies
/// through the same filter — it is always in range of its own kill). Each recipient gets `1/n` of
/// its OWN level-based kill XP ([`crate::xp::award_xp`]'s `share_count`) and full quest
/// kill-credit.
pub(crate) fn kill_reward_recipients(
    ctx: &ReducerContext,
    killer_guid: u64,
    x: f32,
    y: f32,
    map_id: u32,
    instance_id: u64,
) -> Vec<u64> {
    let Some(m) = group_of(ctx, killer_guid) else {
        return vec![killer_guid];
    };
    let entities = ctx.db.game_world_entity();
    members_of(ctx, m.group_id)
        .into_iter()
        .filter_map(|member| {
            let e = entities.guid().find(member.character_guid)?;
            let (dx, dy) = (e.x - x, e.y - y);
            eligible_for_kill_reward(
                e.dead,
                e.map_id == map_id,
                e.instance_id == instance_id,
                dx * dx + dy * dy,
            )
            .then_some(member.character_guid)
        })
        .collect()
}

/// A single group member's kill-reward eligibility: alive, on the kill's map AND instance
/// (`same_instance` — work-item 190 slice 1, always true this slice since every entity is
/// instance 0), and within the group-XP range of the corpse (`dist_sq` = squared 2-D distance,
/// compared INCLUSIVELY against [`GROUP_XP_RANGE_SQ`]). The per-member gate of
/// [`kill_reward_recipients`], extracted so the eligibility rules are unit-testable without a
/// `ReducerContext`. Pure.
pub(crate) fn eligible_for_kill_reward(
    dead: bool,
    same_map: bool,
    same_instance: bool,
    dist_sq: f32,
) -> bool {
    !dead && same_map && same_instance && dist_sq <= GROUP_XP_RANGE_SQ
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The share filter's range gate is the vanilla 74yd group-XP distance, squared for the
    /// comparison `kill_reward_recipients` runs — pin it so a tuning edit is a conscious act.
    #[test]
    fn group_xp_range_is_74_yards() {
        assert_eq!(GROUP_XP_RANGE_SQ, 5476.0);
        assert_eq!(GROUP_MAX_MEMBERS, 5);
    }

    #[test]
    fn kill_reward_needs_an_alive_member_on_the_same_map_within_range() {
        // The all-pass baseline: alive, same map, same instance, at the corpse.
        assert!(eligible_for_kill_reward(false, true, true, 0.0));
        // Exactly at the 74yd² boundary is still IN (inclusive comparison).
        assert!(eligible_for_kill_reward(
            false,
            true,
            true,
            GROUP_XP_RANGE_SQ
        ));
        // Each gate rejects on its own: a dead member, another map, another instance, out of range.
        assert!(!eligible_for_kill_reward(true, true, true, 0.0));
        assert!(!eligible_for_kill_reward(false, false, true, 0.0));
        assert!(!eligible_for_kill_reward(false, true, false, 0.0));
        assert!(!eligible_for_kill_reward(
            false,
            true,
            true,
            GROUP_XP_RANGE_SQ + 1.0
        ));
    }

    #[test]
    fn leadership_passes_to_the_longest_standing_member_or_the_group_disbands() {
        // Fewer than 2 remaining → disband (None), whoever led and whoever left.
        assert_eq!(leader_after_removal(&[], 7, 7), None);
        assert_eq!(leader_after_removal(&[(3, 30)], 7, 7), None);
        assert_eq!(leader_after_removal(&[(3, 30)], 7, 30), None); // even a surviving leader disbands alone
                                                                   // A NON-leader leaving keeps the incumbent.
        assert_eq!(leader_after_removal(&[(1, 10), (2, 20)], 99, 10), Some(10));
        // The LEADER leaving passes to the lowest member-ROW id (longest-standing), not the lowest guid.
        assert_eq!(leader_after_removal(&[(5, 90), (9, 20)], 10, 10), Some(90));
        assert_eq!(leader_after_removal(&[(9, 20), (5, 90)], 10, 10), Some(90));
        // order-independent
    }

    // ---- Group loot methods (work-item 187 slice 1) ----

    #[test]
    fn loot_method_encoding_matches_the_real_wire_grouplootsetting_order() {
        // wow_world_base::GroupLootSetting: FreeForAll=0, RoundRobin=1, MasterLoot=2, GroupLoot=3,
        // NeedBeforeGreed=4 — a mismatch here would silently misrender the party frame's loot icon.
        assert_eq!(loot_method::FFA, 0);
        assert_eq!(loot_method::ROUND_ROBIN, 1);
        assert_eq!(loot_method::MASTER, 2);
        assert_eq!(loot_method::GROUP, 3);
        assert_eq!(loot_method::NEED_BEFORE_GREED, 4);
    }

    #[test]
    fn valid_loot_method_admits_only_the_five_known_values() {
        for m in [
            loot_method::FFA,
            loot_method::ROUND_ROBIN,
            loot_method::MASTER,
            loot_method::GROUP,
            loot_method::NEED_BEFORE_GREED,
        ] {
            assert!(valid_loot_method(m), "method {m} should be valid");
        }
        assert!(!valid_loot_method(5));
        assert!(!valid_loot_method(255));
    }

    #[test]
    fn valid_loot_threshold_admits_the_seven_itemquality_values_only() {
        for t in 0u8..=6 {
            assert!(
                valid_loot_threshold(t),
                "quality {t} should be a valid threshold"
            );
        }
        assert!(!valid_loot_threshold(7));
        assert!(!valid_loot_threshold(255));
    }

    // ---- The realm-core mirror (group slice) ----

    #[test]
    fn the_mirror_diff_deletes_departed_rows_and_inserts_arriving_members() {
        // Steady state: the mirror already matches the authority — nothing to do at all. (A diff
        // that re-inserted every member on every push would churn `game_group_member` row ids on
        // every party op, on every shard.)
        let current = [(10u64, 100u64), (11, 200)];
        assert_eq!(mirror_plan(&current, &[100, 200]), (vec![], vec![]));
        // A member joined: only the newcomer is inserted.
        assert_eq!(mirror_plan(&current, &[100, 200, 300]), (vec![], vec![300]));
        // A member left: only their ROW is deleted, addressed by row id (not guid — the delete is
        // applied through the member table's PK).
        assert_eq!(mirror_plan(&current, &[100]), (vec![11], vec![]));
        // A swap does both, and the ROW ID of the departing member is what comes back.
        assert_eq!(mirror_plan(&current, &[200, 300]), (vec![10], vec![300]));
        // Disband: every row goes and nothing arrives.
        assert_eq!(mirror_plan(&current, &[]), (vec![10, 11], vec![]));
        // A shard with no mirror yet (a party crossing a boundary for the first time) inserts all.
        assert_eq!(mirror_plan(&[], &[100, 200]), (vec![], vec![100, 200]));
    }

    // event_recipient_identity's pinned test moved to helpers.rs with the function itself.

    // A reducer body needs a live `ReducerContext`, so the authorization and dispatch decisions
    // below use narrow Architecture Tests.

    /// The `//`-stripped body of `signature`'s function — assert on CODE, never on the prose beside
    /// it. Shared with every other file's copy of this scan as [`crate::test_scan::code_of`]
    /// (this used to be six near-identical, drifted-apart copies).
    use crate::test_scan::code_of;

    /// **The operator gate is the entire authorization of Gateway-driven party work.**
    ///
    /// The claim can consume any invite intent, `realm_group_op` can act as any Character, and
    /// `sync_group_mirror` can replace a whole party roster. Only the Operator may supply those
    /// arguments.
    #[test]
    fn gateway_party_reducers_are_operator_gated() {
        for f in [
            "pub fn claim_bot_invite_intent(",
            "pub fn realm_group_op(",
            "pub fn sync_group_mirror(",
        ] {
            let body = code_of(include_str!("group.rs"), f);
            // The FIRST STATEMENT, not merely present (review). A bare `contains` was
            // satisfied by a gate that never runs: wrapping the line in `if false { … }` — the
            // tripwire defeat this batch has already documented twice — left all 521 module tests
            // green with `realm_group_op` completely ungated, and so would `let _ = …` or an early
            // return above it. Anchoring to the opening brace makes all three visible.
            let normalized: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(
                normalized.starts_with("{ crate::helpers::require_operator(ctx)?;"),
                "`{f}` no longer OPENS with the operator gate. Its arguments authorize party work, \
                 so a gate that is present but neutralized (wrapped in `if false`, `let _ =`, or \
                 preceded by an early return) is no gate. \
                 Body was:\n{body}"
            );
        }
    }

    /// The primary-key delete is the arbitration. A read followed by a second Durable Request
    /// would let two Gateways win against the same row.
    #[test]
    fn the_bot_invite_claim_is_one_atomic_delete() {
        let body = code_of(include_str!("group.rs"), "pub fn claim_bot_invite_intent(");
        let normalized: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            normalized.contains("game_bot_invite_intent().id().delete(intent_id)"),
            "the claim no longer deletes the intent by primary key in its reducer transaction. \
             Body was:\n{body}"
        );
    }

    /// The op byte is a wire value the gateway sends and this reducer dispatches on, and
    /// `lyracore_shared::group::realm_op` pins only the NUMBERS. Nothing pinned what each number DOES
    /// here, so swapping two arms was a silent mis-dispatch — "an ACCEPT arriving as a LEAVE", the
    /// exact failure that shared constant's doc claims to prevent — with every suite green.
    #[test]
    fn every_realm_op_byte_dispatches_to_its_own_core() {
        let body = code_of(include_str!("group.rs"), "pub fn realm_group_op(");
        for (op, core) in [
            (
                "realm_op::INVITE =>",
                "invite_core_on(ctx, Plane::RealmCore, actor_guid, target_guid)",
            ),
            (
                "realm_op::ACCEPT =>",
                "accept_invite_on(ctx, Plane::RealmCore, actor_guid)",
            ),
            ("realm_op::DECLINE =>", "decline_invite_on(ctx, actor_guid)"),
            ("realm_op::LEAVE =>", "leave_group_on(ctx, actor_guid)"),
            (
                "realm_op::UNINVITE =>",
                "uninvite_on(ctx, actor_guid, target_guid)",
            ),
            (
                "realm_op::LOOT_METHOD =>",
                "set_loot_method_on(ctx, actor_guid, arg_a, target_guid, arg_b)",
            ),
        ] {
            let arm = body.split(op).nth(1).unwrap_or_else(|| {
                panic!("`realm_group_op` no longer dispatches `{op}`. Body was:\n{body}")
            });
            // Whitespace-normalised, and a `{` block wrapper stripped: an arm split across lines or
            // wrapped in braces is the same dispatch, and neither should be able to hide a swap.
            let arm: String = arm.split_whitespace().collect::<Vec<_>>().join(" ");
            let arm = arm.strip_prefix("{ ").unwrap_or(&arm);
            assert!(
                arm.starts_with(core),
                "`{op}` no longer runs `{core}`. The op byte is a WIRE value the gateway sends and \
                 this match dispatches on; a swapped arm runs the wrong op for every player, \
                 silently. Arm was:\n{}",
                &arm[..arm.len().min(120)]
            );
        }
    }
}
