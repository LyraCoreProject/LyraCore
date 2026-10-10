//! Mail cache-accessor methods: the mailbox list, and the mailbox-proximity gate.
//!
//! Both read the privileged coordinator cache. Which DATABASE that cache belongs to is the caller's
//! decision, not this file's — `world::mail` asks the realm-core handle when there is one and the
//! session's own handle when there is not, and both reach the same body here.

use std::collections::{BTreeSet, HashMap};

use spacetimedb_sdk::{Table, TableWithPrimaryKey};

use super::super::bindings::*;

/// The mail escrow ids each Character holds on one Shard, kept from the `game_mail_escrow` row
/// callbacks. A drive runs after every quest turn-in, at world entry and at every mailbox visit, so
/// it reads a Character's fences by primary key instead of scanning the cache.
#[derive(Default)]
pub(crate) struct MailEscrowIndex {
    by_sender: HashMap<u64, BTreeSet<u64>>,
}

impl MailEscrowIndex {
    pub(crate) fn insert(&mut self, row: &MailEscrow) {
        self.by_sender
            .entry(row.sender_guid)
            .or_default()
            .insert(row.escrow_id);
    }

    pub(crate) fn remove(&mut self, row: &MailEscrow) {
        if let Some(ids) = self.by_sender.get_mut(&row.sender_guid) {
            ids.remove(&row.escrow_id);
            if ids.is_empty() {
                self.by_sender.remove(&row.sender_guid);
            }
        }
    }

    /// The escrow ids `sender_guid` holds, lowest id first.
    pub(in crate::stdb) fn of(&self, sender_guid: u64) -> Vec<u64> {
        self.by_sender
            .get(&sender_guid)
            .map(|ids| ids.iter().copied().collect())
            .unwrap_or_default()
    }
}

/// Keep a [`MailEscrowIndex`] current from the escrow row callbacks. Initial subscription rows
/// arrive as inserts.
pub(crate) fn watch_mail_escrows(
    conn: &DbConnection,
) -> std::sync::Arc<std::sync::RwLock<MailEscrowIndex>> {
    let index = std::sync::Arc::new(std::sync::RwLock::new(MailEscrowIndex::default()));
    let inserted = index.clone();
    conn.db.game_mail_escrow().on_insert(move |_ctx, row| {
        inserted.write().unwrap().insert(row);
    });
    let updated = index.clone();
    conn.db.game_mail_escrow().on_update(move |_ctx, old, new| {
        let mut escrows = updated.write().unwrap();
        escrows.remove(old);
        escrows.insert(new);
    });
    let deleted = index.clone();
    conn.db.game_mail_escrow().on_delete(move |_ctx, row| {
        deleted.write().unwrap().remove(row);
    });
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    fn escrow(escrow_id: u64, sender_guid: u64) -> MailEscrow {
        MailEscrow {
            escrow_id,
            sender_guid,
            recipient_guid: sender_guid,
            subject: String::new(),
            body: String::new(),
            money: 0,
            postage: 0,
            created_micros: 0,
            delivered: false,
            payout: false,
            mail_id: 0,
            item_entry: 0,
            item_stack_count: 0,
            item_durability: 0,
            item_enchant_id: 0,
            item_soulbound: false,
            cod: 0,
            random_property_id: 0,
            delivery_delay_secs: 0,
            sender_kind: 0,
            sender_entry: 0,
            mail_template_id: 0,
            item_text_id: 0,
        }
    }

    #[test]
    fn the_escrow_index_names_only_the_fences_a_character_still_holds() {
        let mut index = MailEscrowIndex::default();
        index.insert(&escrow(9, 1));
        index.insert(&escrow(4, 1));
        index.insert(&escrow(5, 2));
        assert_eq!(index.of(1), vec![4, 9]);
        index.remove(&escrow(4, 1));
        assert_eq!(index.of(1), vec![9]);
        index.remove(&escrow(9, 1));
        assert!(index.of(1).is_empty());
        assert!(
            !index.by_sender.contains_key(&1),
            "an empty entry is dropped"
        );
        assert_eq!(index.of(2), vec![5]);
    }
}
