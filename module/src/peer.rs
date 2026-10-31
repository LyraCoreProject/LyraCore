use spacetimedb::{reducer, table, Identity, ReducerContext, SpacetimeType, Table};

/// Another Shard of this Realm that may call this Shard's `idc_` reducers.
#[table(accessor = game_peer_shard)]
pub struct PeerShard {
    #[primary_key]
    pub database: Identity,
    pub name: String,
    /// Set once a hello between the two Shards succeeded, which shows that each lists the other.
    pub confirmed: bool,
}

pub(crate) fn is_peer(ctx: &ReducerContext, identity: Identity) -> bool {
    ctx.db.game_peer_shard().database().find(identity).is_some()
}

fn confirm(ctx: &ReducerContext, database: Identity, confirmed: bool) {
    let peers = ctx.db.game_peer_shard();
    if let Some(peer) = peers.database().find(database) {
        peers.database().update(PeerShard { confirmed, ..peer });
    }
}

// A Shard only greets the Shards it lists, so an accepted hello confirms both listings.
#[reducer]
pub fn idc_peer_hello(ctx: &ReducerContext) -> Result<(), String> {
    if !is_peer(ctx, ctx.sender()) {
        return Err("caller is not a Peer Shard".into());
    }
    confirm(ctx, ctx.sender(), true);
    Ok(())
}

#[table(accessor = game_peer_hello, outbox(crate::remote_bindings::peer::idc_peer_hello))]
pub struct PeerHello {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: crate::remote_bindings::peer::Identity,
}

#[reducer(on_result(game_peer_hello))]
pub fn peer_hello_answered(ctx: &ReducerContext, hello: PeerHello, result: Result<(), String>) {
    confirm(ctx, hello.database.0, result.is_ok());
}

/// Lists `database` as a Peer Shard and sends it a hello, which confirms the listing once that
/// Shard lists this one too.
#[reducer]
pub fn operator_add_peer_shard(
    ctx: &ReducerContext,
    database: Identity,
    name: String,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if database == ctx.database_identity() {
        return Err("a Shard cannot be its own Peer Shard".into());
    }
    let peers = ctx.db.game_peer_shard();
    peers.database().delete(database);
    peers.insert(PeerShard {
        database,
        name,
        confirmed: false,
    });
    ctx.db.game_peer_hello().insert(PeerHello {
        id: 0,
        database: crate::remote_bindings::peer::Identity(database),
    });
    Ok(())
}

#[reducer]
pub fn operator_remove_peer_shard(ctx: &ReducerContext, database: Identity) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if !ctx.db.game_peer_shard().database().delete(database) {
        return Err("not a Peer Shard".into());
    }
    Ok(())
}

/// What this Shard holds of its Realm. Without a row the Shard holds the whole Realm.
#[derive(SpacetimeType, Clone, Copy, PartialEq)]
pub enum ShardRole {
    /// Logon, Accounts, Sessions and the Login Queue. Character selection goes to `world`.
    Realm { world: Identity },
    /// Characters and the world. World Sessions arrive from `realm` by Handoff.
    World { realm: Identity },
}

#[table(accessor = game_shard_role)]
pub struct ShardRoleRow {
    #[primary_key]
    pub id: u8,
    pub role: ShardRole,
}

pub(crate) fn role(ctx: &ReducerContext) -> Option<ShardRole> {
    ctx.db.game_shard_role().id().find(0).map(|row| row.role)
}

/// The World Shard of this Realm Shard.
pub(crate) fn world_shard(ctx: &ReducerContext) -> Option<Identity> {
    match role(ctx) {
        Some(ShardRole::Realm { world }) => Some(world),
        _ => None,
    }
}

/// The Realm Shard of this World Shard.
pub(crate) fn realm_shard(ctx: &ReducerContext) -> Option<Identity> {
    match role(ctx) {
        Some(ShardRole::World { realm }) => Some(realm),
        _ => None,
    }
}

/// Refuses a caller other than `expected`, the Shard this role names.
pub(crate) fn require_sender(
    ctx: &ReducerContext,
    expected: Option<Identity>,
) -> Result<(), String> {
    if expected.is_some_and(|expected| expected == ctx.sender() && is_peer(ctx, expected)) {
        Ok(())
    } else {
        Err("caller is not a Peer Shard".into())
    }
}

#[reducer]
pub fn operator_set_shard_role(
    ctx: &ReducerContext,
    role: Option<ShardRole>,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let roles = ctx.db.game_shard_role();
    roles.id().delete(0);
    let Some(role) = role else {
        return Ok(());
    };
    let (ShardRole::Realm { world: peer } | ShardRole::World { realm: peer }) = role;
    if !is_peer(ctx, peer) {
        return Err("the Shard a role names must be a Peer Shard".into());
    }
    roles.insert(ShardRoleRow { id: 0, role });
    Ok(())
}
