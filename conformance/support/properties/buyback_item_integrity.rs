use std::collections::{BTreeMap, BTreeSet};

use super::{Projection, Violation};

pub const NAME: &str = "buyback_item_integrity";

fn links(fields: &BTreeMap<u16, u32>, indices: impl Iterator<Item = u16>) -> BTreeSet<u64> {
    indices
        .filter_map(|index| {
            let guid = u64::from(fields.get(&index).copied().unwrap_or(0))
                | (u64::from(fields.get(&(index + 1)).copied().unwrap_or(0)) << 32);
            (guid != 0).then_some(guid)
        })
        .collect()
}

fn inventory(recipient: u64, projections: &BTreeMap<u64, Projection>) -> BTreeSet<u64> {
    let mut items = projections
        .get(&recipient)
        .map_or_else(BTreeSet::new, |projection| {
            links(
                &projection.fields,
                (486..624).step_by(2).chain((648..712).step_by(2)),
            )
        });
    for projection in projections.values().filter(|projection| {
        matches!(projection.object_type, 1 | 2) && projection.owner_guid == recipient
    }) {
        items.extend(links(&projection.fields, (50..86).step_by(2)));
    }
    items
}

fn buyback(recipient: u64, projections: &BTreeMap<u64, Projection>) -> BTreeSet<u64> {
    projections
        .get(&recipient)
        .map_or_else(BTreeSet::new, |projection| {
            links(&projection.fields, (624..648).step_by(2))
        })
}

pub fn check(
    recipient: u64,
    previous: Option<&BTreeMap<u64, Projection>>,
    current: &BTreeMap<u64, Projection>,
) -> Vec<Violation> {
    let inventory_now = inventory(recipient, current);
    let buyback_now = buyback(recipient, current);
    let mut violations = Vec::new();
    let mut report = |guid, detail: &str| {
        violations.push(Violation {
            property: NAME.into(),
            recipient,
            guid,
            field: None,
            detail: detail.into(),
        })
    };
    for &guid in inventory_now.intersection(&buyback_now) {
        report(guid, "buyback Item is also reachable through inventory");
    }
    if let Some(previous) = previous {
        let inventory_before = inventory(recipient, previous);
        let buyback_before = buyback(recipient, previous);
        let removed: Vec<_> = inventory_before
            .difference(&inventory_now)
            .copied()
            .collect();
        let sold: Vec<_> = buyback_now.difference(&buyback_before).copied().collect();
        if let ([original], [sold]) = (removed.as_slice(), sold.as_slice()) {
            if original != sold {
                report(*sold, "a full sale changed the Item GUID");
            }
        }
    }
    violations
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::properties::{Batch, Packet};
    use std::sync::Arc;

    fn batch(id: u64, fields: &[(u16, u32)]) -> Batch {
        let mut body = vec![1, 0, 0, 0, 0, if id == 1 { 2 } else { 0 }, 1, 1];
        if id == 1 {
            body.extend([4, 0]);
        }
        let blocks = fields
            .iter()
            .map(|(field, _)| field / 32 + 1)
            .max()
            .unwrap();
        body.push(blocks as u8);
        for block in 0..blocks {
            let mask = fields
                .iter()
                .filter(|(field, _)| field / 32 == block)
                .fold(0u32, |mask, (field, _)| mask | 1 << (field % 32));
            body.extend(mask.to_le_bytes());
        }
        for (_, value) in fields {
            body.extend(value.to_le_bytes());
        }
        Batch {
            recipient: 1,
            reset: false,
            packets: vec![Packet {
                id,
                opcode: 0xa9,
                body,
            }],
            projections: BTreeMap::from([(
                1,
                Projection {
                    fields: Arc::new(fields.iter().copied().collect()),
                    object_type: 4,
                    ..Projection::default()
                },
            )]),
            creates: BTreeMap::new(),
        }
    }

    #[test]
    fn a_stream_that_replaces_a_full_sale_guid_violates_item_integrity() {
        let violations =
            crate::properties::run(&[batch(1, &[(536, 10)]), batch(2, &[(536, 0), (624, 20)])])
                .unwrap();
        assert!(violations
            .iter()
            .any(|violation| violation.property == NAME));
    }

    #[test]
    fn a_stream_that_leaves_buyback_in_the_backpack_violates_item_integrity() {
        let violations = crate::properties::run(&[batch(1, &[(536, 10), (624, 10)])]).unwrap();
        assert!(violations
            .iter()
            .any(|violation| violation.property == NAME));
    }

    #[test]
    fn a_stream_that_leaves_buyback_in_an_owned_container_violates_item_integrity() {
        for object_type in [1, 2] {
            let mut batch = batch(1, &[(524, 42), (624, 10)]);
            batch.projections.insert(
                42,
                Projection {
                    fields: Arc::new(BTreeMap::from([(50, 10)])),
                    object_type,
                    owner_guid: 1,
                    ..Projection::default()
                },
            );
            let violations = crate::properties::run(&[batch]).unwrap();
            assert!(violations
                .iter()
                .any(|violation| violation.property == NAME));
        }
    }

    #[test]
    fn a_full_sale_preserves_identity_and_a_partial_sale_may_clone() {
        for fields in [vec![(536, 0), (624, 10)], vec![(536, 10), (624, 20)]] {
            let violations =
                crate::properties::run(&[batch(1, &[(536, 10)]), batch(2, &fields)]).unwrap();
            assert!(violations
                .iter()
                .all(|violation| violation.property != NAME));
        }
    }
}
