mod client_view_consistency;
mod create_kind_matches_freshness;
mod private_fields_stay_private;

use std::collections::BTreeMap;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use wire_client::decode::{self, Field};

pub use client_view_consistency::ClientViews;

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct Violation {
    pub property: String,
    pub recipient: u64,
    pub guid: u64,
    pub field: Option<u16>,
    pub detail: String,
}

#[derive(Clone, Debug, Default)]
pub struct Projection {
    pub fields: BTreeMap<u16, u32>,
    pub object_type: u8,
    pub owner_guid: u64,
    pub same_raid: bool,
    pub special_info: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct CreateContext {
    pub newly_added: bool,
    pub layer: u8,
    pub gameobject_type: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    pub id: u64,
    pub opcode: u16,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct Batch {
    pub recipient: u64,
    pub reset: bool,
    pub packets: Vec<Packet>,
    pub projections: BTreeMap<u64, Projection>,
    pub creates: BTreeMap<u64, CreateContext>,
}

#[derive(Clone, Debug)]
pub struct Update {
    pub kind: u8,
    pub guid: u64,
    pub object_type: Option<u8>,
    pub fields: BTreeMap<u16, u32>,
}

fn unsigned(field: &Field) -> Result<u64> {
    match field {
        Field::Unsigned(value) | Field::Guid(value) => Ok(*value),
        _ => bail!("expected unsigned update field"),
    }
}

pub fn updates(opcode: u16, body: &[u8]) -> Result<Vec<Update>> {
    if !matches!(opcode, 0x00a9 | 0x01f6) {
        return Ok(Vec::new());
    }
    let decoded = decode::decode(opcode, body)?;
    let Field::List(objects) = decoded.fields.get("objects").context("update objects")? else {
        bail!("update objects are not a list");
    };
    let mut updates = Vec::new();
    for object in objects {
        let Field::Object(object) = object else {
            bail!("update object is malformed");
        };
        let kind: u8 = unsigned(object.get("kind").context("update kind")?)?.try_into()?;
        if matches!(kind, 4 | 5) {
            let Field::List(guids) = object.get("guids").context("out of range GUIDs")? else {
                bail!("out of range GUIDs are malformed");
            };
            for guid in guids {
                updates.push(Update {
                    kind,
                    guid: unsigned(guid)?,
                    object_type: None,
                    fields: BTreeMap::new(),
                });
            }
            continue;
        }
        let fields = match object.get("values") {
            Some(Field::Object(values)) => values
                .iter()
                .map(|(name, value)| {
                    Ok((
                        name.strip_prefix("field_")
                            .context("update field name")?
                            .parse()?,
                        unsigned(value)?.try_into()?,
                    ))
                })
                .collect::<Result<BTreeMap<_, _>>>()?,
            None => BTreeMap::new(),
            _ => bail!("update values are malformed"),
        };
        updates.push(Update {
            kind,
            guid: unsigned(object.get("guid").context("update GUID")?)?,
            object_type: object
                .get("object_type")
                .map(|value| Ok::<u8, anyhow::Error>(u8::try_from(unsigned(value)?)?))
                .transpose()?,
            fields,
        });
    }
    Ok(updates)
}

pub fn check_private(recipient: u64, update: &Update, projection: &Projection) -> Vec<Violation> {
    private_fields_stay_private::check(recipient, update, projection)
}

pub fn check_create_kind(
    recipient: u64,
    update: &Update,
    context: Option<CreateContext>,
) -> Vec<Violation> {
    create_kind_matches_freshness::check(recipient, update, context)
}

pub fn run(batches: &[Batch]) -> Result<Vec<Violation>> {
    let mut views = BTreeMap::<u64, ClientViews>::new();
    let mut known = BTreeMap::<(u64, u64), Projection>::new();
    let mut violations = Vec::new();
    for batch in batches {
        if batch.reset {
            views.remove(&batch.recipient);
            known.retain(|(recipient, _), _| *recipient != batch.recipient);
        }
        for (&guid, projection) in &batch.projections {
            known.insert((batch.recipient, guid), projection.clone());
        }
        let view = views.entry(batch.recipient).or_default();
        for packet in &batch.packets {
            for update in updates(packet.opcode, &packet.body)? {
                if matches!(update.kind, 0 | 2 | 3) {
                    if let Some(projection) = known.get(&(batch.recipient, update.guid)) {
                        violations.extend(check_private(batch.recipient, &update, projection));
                    } else {
                        violations.push(Violation {
                            property: private_fields_stay_private::NAME.into(),
                            recipient: batch.recipient,
                            guid: update.guid,
                            field: None,
                            detail: "update has no visibility context".into(),
                        });
                    }
                }
                violations.extend(check_create_kind(
                    batch.recipient,
                    &update,
                    batch.creates.get(&packet.id).copied(),
                ));
                view.apply(&update);
            }
            if matches!(packet.opcode, 0x00aa | 0x0215) {
                let decoded = decode::decode(packet.opcode, &packet.body)?;
                if let Some(guid) = decoded.fields.get("guid") {
                    view.remove(unsigned(guid)?);
                }
            }
        }
        violations.extend(view.check(batch.recipient, &batch.projections));
    }
    violations.sort();
    violations.dedup();
    Ok(violations)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(id: u64, kind: u8, guid: u8, object_type: u8, fields: &[(u16, u32)]) -> Packet {
        let mut body = vec![1, 0, 0, 0, 0, kind, 1, guid];
        if kind >= 2 {
            body.extend([object_type, 0]);
        }
        let blocks = fields
            .iter()
            .map(|(field, _)| field / 32 + 1)
            .max()
            .unwrap_or(0);
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
        Packet {
            id,
            opcode: 0x00a9,
            body,
        }
    }

    fn batch(packets: Vec<Packet>, object_type: u8, fields: &[(u16, u32)]) -> Batch {
        Batch {
            recipient: 7,
            reset: false,
            packets,
            projections: BTreeMap::from([(
                9,
                Projection {
                    fields: fields.iter().copied().collect(),
                    object_type,
                    ..Default::default()
                },
            )]),
            creates: BTreeMap::from([(
                1,
                CreateContext {
                    newly_added: false,
                    layer: 0,
                    gameobject_type: 0,
                },
            )]),
        }
    }

    #[test]
    fn private_field_in_a_peer_values_packet_fails_the_pass() -> Result<()> {
        let opening = batch(vec![packet(1, 2, 9, 4, &[])], 4, &[]);
        let change = batch(vec![packet(2, 0, 9, 4, &[(1176, 42)])], 4, &[]);
        let violations = run(&[opening, change])?;
        assert!(violations.iter().any(|violation| violation.property
            == "private_fields_stay_private"
            && violation.field == Some(1176)));
        Ok(())
    }

    #[test]
    fn an_omitted_zero_is_reported_before_a_later_update_repairs_it() -> Result<()> {
        let opening = batch(vec![packet(1, 2, 9, 5, &[(14, 1)])], 5, &[(14, 1)]);
        let change = batch(vec![packet(2, 0, 9, 5, &[(8, 4)])], 5, &[(8, 4), (14, 0)]);
        let repair = batch(vec![packet(3, 0, 9, 5, &[(14, 0)])], 5, &[(8, 4), (14, 0)]);
        let violations = run(&[opening, change, repair])?;
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].property, "client_view_consistency");
        assert_eq!(violations[0].field, Some(14));
        Ok(())
    }

    #[test]
    fn an_entirely_missing_update_fails_even_without_a_packet() -> Result<()> {
        let opening = batch(vec![packet(1, 2, 9, 5, &[(14, 1)])], 5, &[(14, 1)]);
        let change = batch(vec![], 5, &[(14, 0)]);
        let violations = run(&[opening, change])?;
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].field, Some(14));
        Ok(())
    }

    #[test]
    fn a_new_creature_encoded_as_known_fails_the_pass() -> Result<()> {
        let mut spawn = batch(vec![packet(1, 2, 9, 3, &[])], 3, &[]);
        spawn.creates.get_mut(&1).unwrap().newly_added = true;
        let violations = run(&[spawn])?;
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].property, "create_kind_matches_freshness");
        Ok(())
    }
}
