//! Bounded Character namespaces for newly created item identities.

use spacetimedb::ReducerContext;

use super::tables::game_item_instance;

const ITEM_PREFIX: u64 = 0x4000 << 48;
const FORMAT_BIT: u64 = 1 << 47;
const SERIAL_BITS: u32 = 11;
const CAPACITY: usize = 1 << SERIAL_BITS;
const MAX_OWNER: u64 = (1 << (47 - SERIAL_BITS)) - 1;

/// Select the complete batch before changing inventory. Trading also calls this before deleting
/// outgoing items, so an incoming item cannot reuse a GUID deleted by the same Trade Commit.
pub(crate) fn allocate_item_guids(
    ctx: &ReducerContext,
    owner_guid: u64,
    count: usize,
) -> Result<Vec<u64>, String> {
    select_item_guids(owner_guid, count, |guid| {
        ctx.db.game_item_instance().guid().find(guid).is_some()
    })
}

pub(crate) fn next_item_guid(ctx: &ReducerContext, owner_guid: u64) -> Result<u64, String> {
    Ok(allocate_item_guids(ctx, owner_guid, 1)?[0])
}

/// Deleted identities may be reused because ownership changes recreate items in the recipient's
/// namespace. Transfer alone preserves GUIDs and moves the entire Character under Escrow.
fn select_item_guids(
    owner_guid: u64,
    count: usize,
    mut occupied: impl FnMut(u64) -> bool,
) -> Result<Vec<u64>, String> {
    if count == 0 {
        return Ok(Vec::new());
    }
    if owner_guid > MAX_OWNER {
        return Err("ITEM_GUID_OWNER_OUT_OF_RANGE".to_owned());
    }
    if count > CAPACITY {
        return Err("ITEM_GUID_EXHAUSTED".to_owned());
    }
    let base = ITEM_PREFIX | FORMAT_BIT | (owner_guid << SERIAL_BITS);
    let mut guids = Vec::with_capacity(count);
    for serial in 0..CAPACITY {
        let guid = base | serial as u64;
        if !occupied(guid) {
            guids.push(guid);
            if guids.len() == count {
                return Ok(guids);
            }
        }
    }
    Err("ITEM_GUID_EXHAUSTED".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn full_character_guid_and_item_prefix_survive_allocation() {
        let mut issued = BTreeSet::new();
        for owner in [0, 1, 16_777_217, 1_000_000_001, 3_000_000_001, MAX_OWNER] {
            let guids = select_item_guids(owner, CAPACITY, |_| false).unwrap();
            for guid in guids {
                assert_eq!(guid >> 48, 0x4000);
                assert_eq!((guid & ((1 << 47) - 1)) >> 11, owner);
                assert!(issued.insert(guid));
            }
        }
        assert_eq!(
            select_item_guids(MAX_OWNER + 1, 1, |_| false).unwrap_err(),
            "ITEM_GUID_OWNER_OUT_OF_RANGE"
        );
        assert!(select_item_guids(u64::MAX, 1, |_| false).is_err());
    }

    #[test]
    fn a_full_block_refuses_without_escaping_to_the_next_character() {
        let base = 0x4000_8000_0000_0800;
        assert_eq!(
            select_item_guids(1, 1, |guid| {
                assert!((base..base + 2048).contains(&guid));
                true
            })
            .unwrap_err(),
            "ITEM_GUID_EXHAUSTED"
        );
        assert!(select_item_guids(1, 2049, |_| false).is_err());
    }

    #[test]
    fn a_batch_skips_existing_items_and_reuses_only_free_identities() {
        let base = 0x4000_8000_0000_0800;
        let guids =
            select_item_guids(1, 2, |guid| guid != base + 17 && guid != base + 2047).unwrap();
        assert_eq!(guids, [base + 17, base + 2047]);
        assert!(select_item_guids(1, 3, |guid| guid != base + 17 && guid != base + 2047).is_err());
    }
}
