use spacetimedb::{reducer, table, Identity, ReducerContext, Table};

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
