use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use super::{Projection, Update, Violation};

pub const NAME: &str = "client_view_consistency";

#[derive(Default)]
pub struct ClientViews {
    objects: BTreeMap<u64, BTreeMap<u16, u32>>,
    compared: BTreeMap<u64, Arc<BTreeMap<u16, u32>>>,
    changed: BTreeSet<u64>,
}

impl ClientViews {
    pub fn remove(&mut self, guid: u64) {
        self.objects.remove(&guid);
    }

    pub fn apply(&mut self, update: &Update) {
        self.changed.insert(update.guid);
        match update.kind {
            2 | 3 => {
                self.objects.insert(update.guid, update.fields.clone());
            }
            0 => {
                if let Some(fields) = self.objects.get_mut(&update.guid) {
                    fields.extend(&update.fields);
                }
            }
            4 => {
                self.objects.remove(&update.guid);
            }
            _ => (),
        }
    }

    pub fn check(
        &mut self,
        recipient: u64,
        projections: &BTreeMap<u64, Projection>,
    ) -> Vec<Violation> {
        let mut violations = Vec::new();
        for &guid in projections.keys() {
            if !self.objects.contains_key(&guid) {
                violations.push(Violation {
                    property: NAME.into(),
                    recipient,
                    guid,
                    field: None,
                    detail: "visible object has no CREATE".into(),
                });
            }
        }
        for (&guid, actual) in &self.objects {
            let Some(expected) = projections.get(&guid) else {
                violations.push(Violation {
                    property: NAME.into(),
                    recipient,
                    guid,
                    field: None,
                    detail: "visible object has no fresh projection".into(),
                });
                continue;
            };
            if !self.changed.contains(&guid)
                && self
                    .compared
                    .get(&guid)
                    .is_some_and(|previous| Arc::ptr_eq(previous, &expected.fields))
            {
                continue;
            }
            let fields = actual
                .keys()
                .chain(
                    expected
                        .fields
                        .keys()
                        .filter(|field| !actual.contains_key(*field)),
                )
                .copied();
            for field in fields {
                let observed = actual.get(&field).copied().unwrap_or(0);
                let fresh = expected.fields.get(&field).copied().unwrap_or(0);
                if observed != fresh {
                    violations.push(Violation {
                        property: NAME.into(),
                        recipient,
                        guid,
                        field: Some(field),
                        detail: format!("client has {observed}, fresh projection has {fresh}"),
                    });
                }
            }
        }
        self.compared = projections
            .iter()
            .map(|(&guid, projection)| (guid, projection.fields.clone()))
            .collect();
        self.changed.clear();
        violations
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omitted_door_state_in_values_fails() {
        let mut views = ClientViews::default();
        views.apply(&Update {
            kind: 2,
            guid: 10,
            object_type: Some(5),
            fields: BTreeMap::from([(14, 1)]),
        });
        views.apply(&Update {
            kind: 0,
            guid: 10,
            object_type: None,
            fields: BTreeMap::from([(8, 4)]),
        });
        let projections = BTreeMap::from([(
            10,
            Projection {
                fields: BTreeMap::from([(8, 4), (14, 0)]).into(),
                ..Default::default()
            },
        )]);
        assert_eq!(views.check(7, &projections)[0].field, Some(14));
    }

    #[test]
    fn visible_object_without_create_fails() {
        let projections = BTreeMap::from([(10, Projection::default())]);
        assert_eq!(ClientViews::default().check(7, &projections)[0].guid, 10);
    }
}
