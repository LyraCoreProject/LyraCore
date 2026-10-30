use super::{Projection, Update, Violation};

pub const NAME: &str = "private_fields_stay_private";

// UpdateFields_1_12_1.h at d9173eca, with inherited OBJECT, ITEM, and UNIT fields.
fn permits(
    field: u16,
    object_type: u8,
    recipient: u64,
    guid: u64,
    projection: &Projection,
) -> bool {
    if field == 5
        || matches!(
            (object_type, field),
            (2, 49) | (3 | 4, 187) | (5, 25) | (7, 37)
        )
    {
        return false;
    }
    if recipient == guid {
        return true;
    }
    let owner = recipient == projection.owner_guid && recipient != 0;
    if matches!(object_type, 1 | 2) {
        return !matches!(field, 14..=20 | 45..=47) || owner;
    }
    if matches!(object_type, 3 | 4) {
        match field {
            128 => return false,
            134..=137 | 155..=161 => return owner || projection.special_info,
            141..=142 | 149..=154 | 162..=163 | 165..=186 => return owner,
            _ => (),
        }
    }
    if object_type == 4 {
        if (198..=257).contains(&field) {
            return field.is_multiple_of(3) && projection.same_raid;
        }
        if (486..=1281).contains(&field) {
            return false;
        }
    }
    true
}

pub fn check(recipient: u64, update: &Update, projection: &Projection) -> Vec<Violation> {
    update
        .fields
        .keys()
        .filter(|&&field| {
            !permits(
                field,
                projection.object_type,
                recipient,
                update.guid,
                projection,
            )
        })
        .map(|&field| Violation {
            property: NAME.into(),
            recipient,
            guid: update.guid,
            field: Some(field),
            detail: "field is not visible to this recipient under the 1.12.1 flags".into(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn padding_is_never_sent_even_to_self_owner_or_raid() {
        for (object_type, field) in [
            (0, 5),
            (1, 5),
            (2, 5),
            (2, 49),
            (3, 187),
            (4, 187),
            (5, 25),
            (6, 5),
            (7, 37),
        ] {
            for recipient in [7, 8, 9] {
                for kind in [0, 2, 3] {
                    let update = Update {
                        kind,
                        guid: 7,
                        object_type: Some(object_type),
                        fields: BTreeMap::from([(field, 0)]),
                    };
                    let projection = Projection {
                        object_type,
                        owner_guid: 8,
                        same_raid: true,
                        special_info: true,
                        ..Default::default()
                    };
                    assert_eq!(
                        check(recipient, &update, &projection).len(),
                        1,
                        "type {object_type}, field {field}, recipient {recipient}, kind {kind}"
                    );
                }
            }
        }
    }

    #[test]
    fn peer_create_with_self_only_fields_fails() {
        let update = Update {
            kind: 2,
            guid: 7,
            object_type: Some(4),
            fields: BTreeMap::from([(716, 5), (1176, 100)]),
        };
        let projection = Projection {
            object_type: 4,
            owner_guid: 7,
            ..Default::default()
        };
        assert_eq!(check(9, &update, &projection).len(), 2);
        assert!(check(7, &update, &projection).is_empty());
    }

    #[test]
    fn pet_owner_receives_owner_only_but_not_private_fields() {
        let pet = Update {
            kind: 0,
            guid: 90,
            object_type: None,
            fields: BTreeMap::from([(128, 2000), (150, 5)]),
        };
        let pet_projection = Projection {
            object_type: 3,
            owner_guid: 7,
            ..Default::default()
        };
        let owner_violations = check(7, &pet, &pet_projection);
        assert_eq!(owner_violations.len(), 1);
        assert_eq!(owner_violations[0].field, Some(128));
    }

    #[test]
    fn raid_member_receives_group_only_but_not_private_quest_fields() {
        let peer = Update {
            kind: 2,
            guid: 8,
            object_type: Some(4),
            fields: BTreeMap::from([(198, 12), (199, 34)]),
        };
        let raid_projection = Projection {
            object_type: 4,
            owner_guid: 8,
            same_raid: true,
            ..Default::default()
        };
        let raid_violations = check(7, &peer, &raid_projection);
        assert_eq!(raid_violations.len(), 1);
        assert_eq!(raid_violations[0].field, Some(199));
    }

    #[test]
    fn item_owner_receives_owner_only_fields() {
        let item = Update {
            kind: 2,
            guid: 100,
            object_type: Some(1),
            fields: BTreeMap::from([(46, 3)]),
        };
        let item_projection = Projection {
            object_type: 1,
            owner_guid: 7,
            ..Default::default()
        };
        assert!(check(7, &item, &item_projection).is_empty());
        assert_eq!(check(9, &item, &item_projection).len(), 1);
    }

    #[test]
    fn peer_player_public_fields_after_unit_end_are_allowed() {
        let player = Update {
            kind: 2,
            guid: 8,
            object_type: Some(4),
            fields: BTreeMap::from([(188, 7), (194, 2)]),
        };
        let projection = Projection {
            object_type: 4,
            owner_guid: 8,
            ..Default::default()
        };
        assert!(check(7, &player, &projection).is_empty());
    }

    #[test]
    fn all_power_cost_multipliers_require_self_or_owner() {
        let update = Update {
            kind: 0,
            guid: 90,
            object_type: None,
            fields: (180..=186).map(|field| (field, 1)).collect(),
        };
        let projection = Projection {
            object_type: 3,
            owner_guid: 7,
            ..Default::default()
        };
        assert_eq!(check(9, &update, &projection).len(), 7);
        assert!(check(7, &update, &projection).is_empty());
    }
}
