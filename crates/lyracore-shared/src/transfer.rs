//! Cross-crate Transfer bounds.

/// Maximum durable session-less Transfer Intents on one World Shard.
///
/// The Module refuses the next insert at this bound. The Gateway can therefore inspect the whole
/// pending set with one bounded read; `limit + 1` detects incompatible populated state.
pub const BOT_TRANSFER_PENDING_LIMIT: usize = 64;
