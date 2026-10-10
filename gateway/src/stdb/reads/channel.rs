//! Chat Channel reads from the Realm-core cache, through [`ChannelIndex`] so a request never scans
//! the channel or member tables.

use std::collections::{BTreeMap, HashMap};

use super::*;

/// The Gateway-side index over `game_chat_channel` and `game_chat_channel_member`, kept current by
/// the cache's insert, update and delete callbacks. The SDK cache has only unique-index finds, and a
/// whole-table scan would copy every row under the lock the pump needs.
#[derive(Default)]
pub(crate) struct ChannelIndex {
    by_name: HashMap<(u32, String), u64>,
    /// Channel id to its members by member row id, which is join order: `(guid, member flags)`.
    members: HashMap<u64, BTreeMap<u64, (u64, u8)>>,
}

impl ChannelIndex {
    pub(crate) fn insert_channel(&mut self, row: &ChatChannel) {
        self.by_name
            .insert((row.team, row.name_key.clone()), row.channel_id);
    }

    pub(crate) fn remove_channel(&mut self, row: &ChatChannel) {
        let key = (row.team, row.name_key.clone());
        if self.by_name.get(&key) == Some(&row.channel_id) {
            self.by_name.remove(&key);
        }
    }

    pub(crate) fn insert_member(&mut self, row: &ChatChannelMember) {
        self.members
            .entry(row.channel_id)
            .or_default()
            .insert(row.id, (row.character_guid, row.member_flags));
    }

    pub(crate) fn remove_member(&mut self, row: &ChatChannelMember) {
        if let Some(members) = self.members.get_mut(&row.channel_id) {
            members.remove(&row.id);
            if members.is_empty() {
                self.members.remove(&row.channel_id);
            }
        }
    }

    pub(in crate::stdb) fn channel_id(&self, team: u32, name_key: &str) -> Option<u64> {
        self.by_name.get(&(team, name_key.to_string())).copied()
    }

    pub(in crate::stdb) fn members(&self, channel_id: u64) -> Vec<(u64, u8)> {
        self.members
            .get(&channel_id)
            .map(|members| members.values().copied().collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(channel_id: u64, team: u32, name_key: &str) -> ChatChannel {
        ChatChannel {
            channel_id,
            team,
            name_key: name_key.to_string(),
            name: name_key.to_string(),
            builtin_id: 0,
            flags: 0x01,
            password: String::new(),
            owner_guid: 0,
            announcements: true,
            moderated: false,
        }
    }

    fn member(id: u64, channel_id: u64, guid: u64, flags: u8) -> ChatChannelMember {
        ChatChannelMember {
            id,
            channel_id,
            character_guid: guid,
            member_flags: flags,
            account_id: 0,
            claim_generation: 0,
        }
    }

    #[test]
    fn a_channel_is_found_by_team_and_key() {
        let mut index = ChannelIndex::default();
        index.insert_channel(&channel(1, 469, "trade - city"));
        index.insert_channel(&channel(2, 67, "trade - city"));
        assert_eq!(index.channel_id(469, "trade - city"), Some(1));
        assert_eq!(index.channel_id(67, "trade - city"), Some(2));
        assert_eq!(index.channel_id(469, "general - x"), None);
        index.remove_channel(&channel(1, 469, "trade - city"));
        assert_eq!(index.channel_id(469, "trade - city"), None);
    }

    #[test]
    fn members_come_back_in_join_order_with_their_current_flags() {
        let mut index = ChannelIndex::default();
        index.insert_member(&member(9, 1, 90, 0));
        index.insert_member(&member(4, 1, 40, 0x03));
        index.insert_member(&member(5, 2, 50, 0));
        assert_eq!(index.members(1), [(40, 0x03), (90, 0)]);
        // An update arrives as remove then insert.
        index.remove_member(&member(9, 1, 90, 0));
        index.insert_member(&member(9, 1, 90, 0x08));
        assert_eq!(index.members(1), [(40, 0x03), (90, 0x08)]);
        index.remove_member(&member(4, 1, 40, 0x03));
        index.remove_member(&member(9, 1, 90, 0x08));
        assert!(index.members(1).is_empty());
        assert_eq!(index.members(2), [(50, 0)]);
    }

    /// A replaced channel's late delete must not unmap the channel now holding its name.
    #[test]
    fn a_stale_channel_delete_keeps_the_current_mapping() {
        let mut index = ChannelIndex::default();
        index.insert_channel(&channel(3, 469, "raiders"));
        index.remove_channel(&channel(1, 469, "raiders"));
        assert_eq!(index.channel_id(469, "raiders"), Some(3));
    }
}
