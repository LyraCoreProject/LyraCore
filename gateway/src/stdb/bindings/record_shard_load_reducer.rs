// HAND-AUTHORED (issue #78) — NOT a `spacetime generate` output. `docs/danger-zones.md` §1.2's
// documented-exception pattern: this reducer binding was spliced by hand rather than regenerating
// all ~512 bindings files, mirroring `install_guid_range_reducer.rs`/`claim_guid_range_reducer.rs`
// exactly (same shape: a private `*Args` struct, a `From<Args> for Reducer` impl, and an extension
// trait implemented for `RemoteReducers`). A future `spacetime generate` overwrites this file with
// byte-identical content; nothing needs undoing.
#![allow(unused, clippy::all)]
use spacetimedb_sdk::__codegen::{self as __sdk, __lib, __sats, __ws};

#[derive(__lib::ser::Serialize, __lib::de::Deserialize, Clone, PartialEq, Debug)]
#[sats(crate = __lib)]
pub(super) struct RecordShardLoadArgs {
    pub shard: String,
    pub writer_occupancy_pct: f32,
    pub sessions: u32,
}

impl From<RecordShardLoadArgs> for super::Reducer {
    fn from(args: RecordShardLoadArgs) -> Self {
        Self::RecordShardLoad {
            shard: args.shard,
            writer_occupancy_pct: args.writer_occupancy_pct,
            sessions: args.sessions,
        }
    }
}

impl __sdk::InModule for RecordShardLoadArgs {
    type Module = super::RemoteModule;
}

#[allow(non_camel_case_types)]
/// Extension trait for access to the reducer `record_shard_load`.
///
/// Implemented for [`super::RemoteReducers`].
pub trait record_shard_load {
    /// Request that the remote module invoke the reducer `record_shard_load` to run as soon as possible.
    ///
    /// This method returns immediately, and errors only if we are unable to send the request.
    /// The reducer will run asynchronously in the future,
    ///  and this method provides no way to listen for its completion status.
    /// /// Use [`record_shard_load:record_shard_load_then`] to run a callback after the reducer completes.
    fn record_shard_load(
        &self,
        shard: String,
        writer_occupancy_pct: f32,
        sessions: u32,
    ) -> __sdk::Result<()> {
        self.record_shard_load_then(shard, writer_occupancy_pct, sessions, |_, _| {})
    }

    /// Request that the remote module invoke the reducer `record_shard_load` to run as soon as possible,
    /// registering `callback` to run when we are notified that the reducer completed.
    ///
    /// This method returns immediately, and errors only if we are unable to send the request.
    /// The reducer will run asynchronously in the future,
    ///  and its status can be observed with the `callback`.
    fn record_shard_load_then(
        &self,
        shard: String,
        writer_occupancy_pct: f32,
        sessions: u32,

        callback: impl FnOnce(&super::ReducerEventContext, Result<Result<(), String>, __sdk::InternalError>)
            + Send
            + 'static,
    ) -> __sdk::Result<()>;
}

impl record_shard_load for super::RemoteReducers {
    fn record_shard_load_then(
        &self,
        shard: String,
        writer_occupancy_pct: f32,
        sessions: u32,

        callback: impl FnOnce(&super::ReducerEventContext, Result<Result<(), String>, __sdk::InternalError>)
            + Send
            + 'static,
    ) -> __sdk::Result<()> {
        self.imp.invoke_reducer_with_callback(
            RecordShardLoadArgs {
                shard,
                writer_occupancy_pct,
                sessions,
            },
            callback,
        )
    }
}
