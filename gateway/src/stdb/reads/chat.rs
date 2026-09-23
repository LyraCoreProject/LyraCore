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

    /// The Speaker Facts for `speaker_guid` on this Home Shard: race from `UNIT_FIELD_BYTES_0`
    /// byte 0 and the chat tag from `PLAYER_FLAGS`, both off the live entity, plus the Character's
    /// name. `None` when the speaker has no live entity here.
    pub(crate) fn speaker_facts(
        &self,
        speaker_guid: u64,
    ) -> anyhow::Result<Option<crate::world::SpeakerFacts>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let Some(entity) = db.game_world_entity().guid().find(&speaker_guid) else {
            return Ok(None);
        };
        let name = db
            .game_character()
            .guid()
            .find(&speaker_guid)
            .map(|character| character.name)
            .unwrap_or_default();
        Ok(Some(crate::world::SpeakerFacts {
            race: (entity.unit_bytes_0 & 0xFF) as u8,
            chat_tag: lyracore_shared::chat::chat_tag_for(entity.player_flags),
            name,
        }))
    }
}
