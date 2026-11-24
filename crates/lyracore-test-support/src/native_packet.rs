use std::collections::BTreeSet;

use serde::de::IgnoredAny;
use serde_json::value::RawValue;

/// One decoded `game_native_packet` row: a Module delivery instruction for the Native Host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativePacket {
    pub id: u64,
    pub audience: NativeAudience,
    pub opcode: Option<u16>,
    pub body: Vec<u8>,
    pub close_after: bool,
    pub not_before_micros: i64,
    pub authentication_complete: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeAudience {
    Connection(u128),
    Aoi {
        layer: u8,
        entity_guid: u64,
        exclude_connection: Option<u128>,
    },
    Grant(NativeRecipient),
    Revoke(NativeRecipient),
    Clear(u128),
    Handoff(u128),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeRecipient {
    pub connection: u128,
    pub layer: u8,
    pub entity_guid: u64,
}

type Row = (
    u64,
    (u8, Box<RawValue>),
    (u8, Box<RawValue>),
    String,
    bool,
    i64,
    bool,
);

impl NativePacket {
    // Rows are positional SATS JSON: sum values are [tag, payload] and ConnectionId is [u128].
    pub(crate) fn decode(row: &str) -> Result<Self, String> {
        let (
            id,
            (audience, recipient),
            (option, opcode),
            body,
            close_after,
            not_before_micros,
            authentication_complete,
        ): Row = parse(row)?;
        let recipient = recipient.get();
        let audience = match audience {
            0 => NativeAudience::Connection(parse::<(u128,)>(recipient)?.0),
            1 => {
                let (layer, entity_guid, (tag, excluded)): (u8, u64, (u8, Box<RawValue>)) =
                    parse(recipient)?;
                let exclude_connection = match tag {
                    0 => Some(parse::<(u128,)>(excluded.get())?.0),
                    1 => None,
                    _ => return Err(format!("invalid excluded connection option {tag}")),
                };
                NativeAudience::Aoi {
                    layer,
                    entity_guid,
                    exclude_connection,
                }
            }
            2 | 3 => {
                let ((connection,), layer, entity_guid): ((u128,), u8, u64) = parse(recipient)?;
                let recipient = NativeRecipient {
                    connection,
                    layer,
                    entity_guid,
                };
                if audience == 2 {
                    NativeAudience::Grant(recipient)
                } else {
                    NativeAudience::Revoke(recipient)
                }
            }
            4 => NativeAudience::Clear(parse::<(u128,)>(recipient)?.0),
            5 => NativeAudience::Handoff(parse::<((u128,), IgnoredAny)>(recipient)?.0 .0),
            _ => return Err(format!("unknown native delivery audience {audience}")),
        };
        let opcode = match option {
            0 => Some(parse(opcode.get())?),
            1 => None,
            _ => return Err(format!("invalid opcode option {option}")),
        };
        Ok(Self {
            id,
            audience,
            opcode,
            body: decode_hex(&body)?,
            close_after,
            not_before_micros,
            authentication_complete,
        })
    }
}

fn parse<'a, T: serde::Deserialize<'a>>(text: &'a str) -> Result<T, String> {
    serde_json::from_str(text).map_err(|error| format!("invalid native packet row {text}: {error}"))
}

fn decode_hex(encoded: &str) -> Result<Vec<u8>, String> {
    if !encoded.is_ascii() || !encoded.len().is_multiple_of(2) {
        return Err(format!("invalid packet hex {encoded}"));
    }
    (0..encoded.len())
        .step_by(2)
        .map(|offset| {
            u8::from_str_radix(&encoded[offset..offset + 2], 16)
                .map_err(|_| format!("invalid packet hex {encoded}"))
        })
        .collect()
}

/// Routes packets to one World Session as the Native Host does, ending at a Handoff marker.
pub struct WorldSessionObserver {
    connection: u128,
    grants: BTreeSet<(u8, u64)>,
    handed_off: bool,
}

impl WorldSessionObserver {
    pub fn new(connection: u128) -> Self {
        Self {
            connection,
            grants: BTreeSet::new(),
            handed_off: false,
        }
    }

    /// Applies one packet's AOI instruction and returns whether this World Session receives it.
    pub fn receives(&mut self, packet: &NativePacket) -> bool {
        if self.handed_off {
            return false;
        }
        match packet.audience {
            NativeAudience::Connection(connection) => connection == self.connection,
            NativeAudience::Handoff(connection) => {
                self.handed_off = connection == self.connection;
                self.handed_off
            }
            NativeAudience::Aoi {
                layer,
                entity_guid,
                exclude_connection,
            } => {
                self.grants.contains(&(layer, entity_guid))
                    && exclude_connection != Some(self.connection)
            }
            NativeAudience::Grant(recipient) | NativeAudience::Revoke(recipient) => {
                if recipient.connection != self.connection {
                    return false;
                }
                let subject = (recipient.layer, recipient.entity_guid);
                if matches!(packet.audience, NativeAudience::Grant(_)) {
                    self.grants.insert(subject);
                } else {
                    self.grants.remove(&subject);
                }
                true
            }
            NativeAudience::Clear(connection) => {
                if connection == self.connection {
                    self.grants.clear();
                }
                false
            }
        }
    }

    /// Routes one batch in packet id order, as the Host orders a transaction.
    pub fn received(&mut self, packets: &[NativePacket]) -> Vec<NativePacket> {
        let mut ordered: Vec<&NativePacket> = packets.iter().collect();
        ordered.sort_by_key(|packet| packet.id);
        ordered
            .into_iter()
            .filter(|packet| self.receives(packet))
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(audience: &str) -> NativePacket {
        NativePacket::decode(&format!(
            "[1,{audience},[0,477],\"07000000\",false,0,false]"
        ))
        .unwrap()
    }

    #[test]
    fn rows_decode_every_audience_option_and_delivery_field() {
        let row = NativePacket::decode(&format!(
            "[9,[1,[0,55,[0,[{}]]]],[1,[]],\"\",true,40,true]",
            u128::MAX
        ))
        .unwrap();
        assert_eq!(
            row,
            NativePacket {
                id: 9,
                audience: NativeAudience::Aoi {
                    layer: 0,
                    entity_guid: 55,
                    exclude_connection: Some(u128::MAX),
                },
                opcode: None,
                body: Vec::new(),
                close_after: true,
                not_before_micros: 40,
                authentication_complete: true,
            }
        );
        assert_eq!(packet("[5,[[3],[7]]]").audience, NativeAudience::Handoff(3));
        assert_eq!(packet("[4,[3]]").audience, NativeAudience::Clear(3));
        assert_eq!(packet("[0,[3]]").body, [7, 0, 0, 0]);
    }

    #[test]
    fn malformed_rows_fail_instead_of_losing_evidence() {
        for row in [
            "[1,[9,[]],[1,[]],\"\",false,0,false]",
            "[1,[1,[0,55,[2,[]]]],[0,477],\"00\",false,0,false]",
            "[1,[0,[1]],[2,[]],\"00\",false,0,false]",
            "[1,[0,[1]],[0,477],\"xx\",false,0,false]",
            "[1,[0,[1]],[0,477],\"0\",false,0,false]",
            "[truncated",
        ] {
            assert!(NativePacket::decode(row).is_err(), "{row}");
        }
    }

    #[test]
    fn a_world_session_follows_grants_revoke_clear_and_excluded_connections() {
        let mut session = WorldSessionObserver::new(u128::MAX);
        let broadcast = packet("[1,[0,55,[1,[]]]]");
        assert!(!session.receives(&broadcast));
        let grant = packet(&format!("[2,[[{}],0,55]]", u128::MAX));
        assert!(session.receives(&grant));
        assert!(session.receives(&broadcast));
        assert!(!session.receives(&packet("[1,[1,55,[1,[]]]]")));
        assert!(!session.receives(&packet(&format!("[1,[0,55,[0,[{}]]]]", u128::MAX))));
        assert!(!session.receives(&packet("[0,[1]]")));
        assert!(!session.receives(&packet("[2,[[1],0,56]]")));
        assert!(session.receives(&packet(&format!("[3,[[{}],0,55]]", u128::MAX))));
        assert!(!session.receives(&broadcast));
        session.receives(&grant);
        assert!(!session.receives(&packet(&format!("[4,[{}]]", u128::MAX))));
        assert!(!session.receives(&broadcast));
    }

    #[test]
    fn a_handoff_marker_is_the_last_packet_a_world_session_receives() {
        let mut session = WorldSessionObserver::new(1);
        let packets = [
            r#"[1,[2,[[1],0,55]],[0,169],"01",false,0,false]"#,
            r#"[2,[5,[[2],[7]]],[1,[]],"",false,0,false]"#,
            r#"[3,[0,[1]],[0,477],"03",false,0,false]"#,
            r#"[4,[5,[[1],[7]]],[1,[]],"",false,0,false]"#,
            r#"[5,[0,[1]],[0,477],"05",false,0,false]"#,
            r#"[6,[1,[0,55,[1,[]]]],[0,169],"06",false,0,false]"#,
        ]
        .map(|row| NativePacket::decode(row).unwrap());
        assert_eq!(
            session
                .received(&packets)
                .iter()
                .map(|packet| packet.id)
                .collect::<Vec<_>>(),
            [1, 3, 4]
        );
        assert!(session.received(&packets[4..]).is_empty());
    }

    #[test]
    fn a_world_session_applies_grants_in_packet_id_order() {
        let mut session = WorldSessionObserver::new(1);
        let packets = [
            r#"[3,[1,[0,55,[1,[]]]],[0,477],"03",false,0,false]"#,
            r#"[1,[1,[0,55,[1,[]]]],[0,477],"01",false,0,false]"#,
            r#"[2,[2,[[1],0,55]],[0,169],"02",false,0,false]"#,
        ]
        .map(|row| NativePacket::decode(row).unwrap());
        assert_eq!(
            session
                .received(&packets)
                .iter()
                .map(|packet| (packet.id, packet.opcode, packet.body.clone()))
                .collect::<Vec<_>>(),
            [(2, Some(169), vec![2]), (3, Some(477), vec![3])]
        );
    }
}
