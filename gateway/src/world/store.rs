//! `WorldStore`: the umbrella over every Store family a World Session reaches, plus the shard
//! routing and session families. Each other family trait sits beside the handler that calls it, so
//! a handler names only the families it needs and a family Fake implements only its own trait.

use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorldSessionToken {
    pub account_id: u64,
    pub generation: u64,
    pub request_nonce: u128,
}

/// Every Store family a World Session reaches. The blanket impl below supplies it, so a Store
/// implements the families and never this trait.
pub trait WorldStore:
    ShardRoutingStore
    + SessionStore
    + CharacterStore
    + TransferStore
    + PartyStore
    + LootWindowStore
    + LootRollStore
    + MailStore
    + SocialStore
    + NpcStore
    + TrainerStore
    + BankStore
    + CombatStore
    + DeathStore
    + TradeStore
    + AuctionActionStore
    + CastStore
    + ChannelActionStore
    + ChatActionStore
    + SpeechStore
    + DuelActionStore
    + GuildActionStore
    + ItemActionStore
    + MeetingStoneActionStore
    + MeleeActionStore
    + MemberStatsStore
    + QuestActionStore
    + TaxiActionStore
    + VendorActionStore
    + WeatherStore
    + Send
    + Sync
{
}

impl<T> WorldStore for T where
    T: ShardRoutingStore
        + SessionStore
        + CharacterStore
        + TransferStore
        + PartyStore
        + LootWindowStore
        + LootRollStore
        + MailStore
        + SocialStore
        + NpcStore
        + TrainerStore
        + BankStore
        + CombatStore
        + DeathStore
        + TradeStore
        + AuctionActionStore
        + CastStore
        + ChannelActionStore
        + ChatActionStore
        + SpeechStore
        + DuelActionStore
        + GuildActionStore
        + ItemActionStore
        + MeetingStoneActionStore
        + MeleeActionStore
        + MemberStatsStore
        + QuestActionStore
        + TaxiActionStore
        + VendorActionStore
        + WeatherStore
        + Send
        + Sync
{
}

/// Shard routing: the handle that serves a Character, a location, Realm-core or every World Shard.
pub(crate) trait ShardRoutingStore: Send + Sync {
    /// The database this handle targets — routing identity, for logs and for the tests that assert
    /// no call ever escapes the player's home shard.
    fn shard_name(&self) -> &str;

    /// Put `character_guid` on the shard that owns its location, running the escrowed transfer if
    /// it is somewhere else, then answer with that shard's handle, or `None` when this handle
    /// already serves it.
    /// Called at every world entry; `Err` fails the login rather than letting a half-moved
    /// character into the world on either side.
    fn settle_home_shard(
        &self,
        character_guid: u64,
    ) -> Result<Option<std::sync::Arc<dyn WorldStore>>>;

    /// The handle for the shard the Shard Map gives `(map_id, instance_id)`, asked of the handle
    /// that currently HOLDS the character (so a live dungeon run stays on its pool member).
    /// `None` = this handle already serves that location, which is what a single-database gateway
    /// always answers.
    ///
    /// [`settle_home_shard`](Self::settle_home_shard) asks the same question from the character's
    /// own row; this asks it about a location nobody is at yet, which is what a session-less
    /// crossing needs before it has anything to route from.
    fn shard_for_location(
        &self,
        map_id: u32,
        instance_id: u64,
    ) -> Option<std::sync::Arc<dyn WorldStore>>;

    /// Bind this shard's per-player connection identity to the account (`establish_session`), so
    /// `player_login` can resolve the caller here. A no-op on the realm shard, where the logon tier
    /// already did it. Called at world entry whenever the session's home shard is not the realm.
    fn bind_shard_session(&self, account_id: u64, session_key: &[u8; 40]) -> Result<()>;

    /// The **realm-core** handle: the database that owns party membership realm-wide.
    ///
    /// `None` is not "no realm-core configured" — it is "this gateway runs against ONE database", in
    /// which case that database already is the authority and there is nothing to route. `world::party`
    /// branches on exactly this, so the single-database path never reads a row it did not read before.
    fn realm_store(&self) -> Option<std::sync::Arc<dyn WorldStore>>;

    /// Realm-core for deleted Character cleanup. An unavailable configured Realm-core is an
    /// infrastructure failure and cannot fall back to a World Shard.
    fn party_cleanup_realm(&self) -> Result<Option<std::sync::Arc<dyn WorldStore>>>;

    /// Realm-core for companion command authority. Configured outages fail closed.
    fn party_command_realm(&self) -> Result<Option<std::sync::Arc<dyn WorldStore>>>;

    /// Realm-core for Transfer locator authority. Only an unsharded Realm uses the local Store;
    /// an unavailable configured Realm-core is an infrastructure failure.
    fn transfer_realm(&self) -> Result<Option<std::sync::Arc<dyn WorldStore>>>;

    /// Every connected WORLD shard's handle (realm-core excluded — it owns no gameplay reads). The
    /// fan-out set for the roster mirror; empty on a single-database gateway, which is what makes the
    /// mirror push a no-op there.
    fn world_stores(&self) -> Vec<std::sync::Arc<dyn WorldStore>>;

    /// Every configured World Shard for command receipt and holder admission. Missing or unhealthy
    /// members are an infrastructure error because absence cannot be certified on a partial set.
    fn party_command_worlds(&self) -> Result<Vec<std::sync::Arc<dyn WorldStore>>>;
}

/// The World Session lifecycle: handshake lookup, Account Claim, world entry, movement and logout reads.
pub(crate) trait SessionStore: Send + Sync {
    /// Look up the shared session key K (+ account id) for an (already uppercased) account
    /// name. `None` when no live session exists for that account (reject the handshake).
    fn lookup_session(&self, account_name: &str) -> Result<Option<WorldSession>>;

    /// Undelivered private System Messages addressed to this character, oldest-first. A Package
    /// `on_login` hook emits its message INSIDE `player_login`, before the session is in the
    /// viewer registry, so the live insert relay has nobody to address — world entry replays what
    /// is still parked in the shard cache.
    fn pending_system_messages(&self, self_guid: u64) -> Vec<String>;

    /// Enter the world with `character_guid`: calls the `player_login` reducer and
    /// returns the live entity to spawn (from the resulting `game_world_entity` row). Errors if
    /// the character isn't the caller's. `entry` picks the reducer: a world-port keeps the Away
    /// Status, a fresh login ends it.
    fn player_login(
        &self,
        account_id: u64,
        character_guid: u64,
        entry: codec::WorldEntry,
    ) -> Result<codec::EntityView>;

    /// Enqueue an accepted inbound movement on this shard's shared movement batch. The live store
    /// serializes `info` once and preserves the mover, opcode, position, orientation, and timestamp
    /// in the queued entry. Relayed peer events arrive back through the shared dispatch.
    fn movement_update(
        &self,
        account_id: u64,
        self_guid: u64,
        opcode: u32,
        info: &MovementInfo,
    ) -> Result<()>;

    /// Subscribe this player's connection to its per-player views (nearby `game_world_entity`,
    /// addressed `game_movement_event`) and push the resulting peer-spawn / movement-relay / destroy
    /// SMSG onto `tx`. The returned guard tears the subscription + callbacks down on
    /// drop. Called once, at `CMSG_PLAYER_LOGIN`, when `self_guid` is known.
    ///
    /// `arrival` is the entity the login batch was just built from, so the viewer's map, partition,
    /// zone and AOI anchor cannot drift from what the client was told. Its zone in particular seeds
    /// the weather routing with the zone world entry already sent weather for, so the first live
    /// crossing is a real crossing rather than the viewer learning its own starting zone.
    fn subscribe_player_events(
        &self,
        account_id: u64,
        self_guid: u64,
        arrival: &codec::EntityView,
        tx: SessionTx,
    ) -> Result<PlayerSubscriptions>;

    /// Forward a parsed addon-bridge command to the module's `client_command` reducer ON
    /// THE PLAYER'S CONNECTION — the handler runs with exactly the player's reducer authority.
    fn client_command(
        &self,
        account_id: u64,
        self_guid: u64,
        cmd: String,
        payload: String,
    ) -> Result<()>;

    /// Is `guid`'s live entity currently in the world? The WORLDPORT_ACK gate: a cross-map
    /// transfer despawns the entity until the ack rebuilds it, so
    /// ABSENT = a transfer is genuinely pending; PRESENT = the ack is spurious (double-send or
    /// crafted) and must be ignored — honoring it would tear down and rebuild a live player
    /// (visible blink, gateway combat-bookkeeping reset) at zero cost to the client.
    fn entity_in_world(&self, guid: u64) -> bool;

    /// The live entity's max health (0 if not in world) — the fall-damage flavor line folds
    /// the shared curve against it.
    fn entity_max_health(&self, guid: u64) -> u32;

    /// Acquire Realm-core ownership and fence every configured World Shard before returning.
    fn claim_session(&self, account_id: u64, character_guid: u64) -> Result<WorldSessionToken>;

    /// Bind subsequent requests to this World Session. `None` keeps the current handle.
    fn bind_session(
        &self,
        token: WorldSessionToken,
    ) -> Result<Option<std::sync::Arc<dyn WorldStore>>>;

    /// Close the matching Shard fences before releasing the Realm-core claim. Stale cleanup is inert.
    fn release_session(&self, token: WorldSessionToken) -> Result<()>;

    /// Return the `combat_until_ms` timestamp for `player_guid`'s entity row (0 if the entity is not
    /// found). Used by the logout handler to deny `CMSG_LOGOUT_REQUEST` while the player is in combat.
    fn player_combat_until_ms(&self, player_guid: u64) -> u64;
}
