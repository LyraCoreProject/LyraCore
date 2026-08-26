use super::*;

impl super::super::connection::Coordinator {
    /// Undelivered private System Messages addressed to `self_guid`, oldest-first, from the
    /// shard's `game_system_message_event` cache. Rows live until the module's event GC reaps
    /// them, so a message emitted inside `player_login` is still here at world entry.
    pub fn system_messages_for(&self, self_guid: u64) -> Vec<String> {
        let mut rows: Vec<(u64, String)> = self
            .0
            .coord()
            .conn
            .db
            .game_system_message_event()
            .iter()
            .filter(|row| row.recipient_guid == self_guid)
            .map(|row| (row.id, row.message))
            .collect();
        rows.sort_unstable_by_key(|(id, _)| *id);
        rows.into_iter().map(|(_, message)| message).collect()
    }
}
