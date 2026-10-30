use super::{CreateContext, Update, Violation};

pub const NAME: &str = "create_kind_matches_freshness";

pub fn check(recipient: u64, update: &Update, context: Option<CreateContext>) -> Vec<Violation> {
    if !matches!(update.kind, 2 | 3) {
        return Vec::new();
    }
    let expected = match context {
        Some(context) => {
            if context.newly_added
                && context.layer != 4
                && update.object_type != Some(4)
                && !(context.layer == 1 && matches!(context.gameobject_type, 11 | 15))
            {
                3
            } else {
                2
            }
        }
        None if update.guid == recipient || matches!(update.object_type, Some(1 | 2)) => 2,
        None => {
            return vec![Violation {
                property: NAME.into(),
                recipient,
                guid: update.guid,
                field: None,
                detail: "peer CREATE has no freshness context".into(),
            }];
        }
    };
    if update.kind == expected {
        Vec::new()
    } else {
        vec![Violation {
            property: NAME.into(),
            recipient,
            guid: update.guid,
            field: None,
            detail: format!("CREATE kind {} requires kind {expected}", update.kind),
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn new_creature_sent_as_known_fails() {
        let update = Update {
            kind: 2,
            guid: 8,
            object_type: Some(3),
            fields: BTreeMap::new(),
        };
        assert_eq!(
            check(
                7,
                &update,
                Some(CreateContext {
                    newly_added: true,
                    layer: 0,
                    gameobject_type: 0,
                })
            )
            .len(),
            1
        );
    }

    #[test]
    fn known_objects_and_transport_exceptions_require_kind_two() {
        for (object_type, layer, gameobject_type, newly_added) in [
            (3, 0, 0, false),
            (4, 0, 0, true),
            (5, 1, 11, true),
            (5, 1, 15, true),
            (5, 4, 0, true),
        ] {
            let context = Some(CreateContext {
                newly_added,
                layer,
                gameobject_type,
            });
            let mut update = Update {
                kind: 3,
                guid: 8,
                object_type: Some(object_type),
                fields: BTreeMap::new(),
            };
            assert_eq!(check(7, &update, context).len(), 1);
            update.kind = 2;
            assert!(check(7, &update, context).is_empty());
        }
    }
}
