use lyracore_protocol::{messages::vanilla::opcodes::ServerOpcodeMessage, world::Packet};

use crate::{actor::ActionRefusal, Refusal};

/// How a native request ends without committing; see docs/architecture.md#native-host-migration.
pub(super) enum Rollback {
    /// The Host sends `replies` after the rollback and the World Session continues.
    Refusal { replies: Vec<Packet>, cause: String },
    /// The Host ends the World Session.
    Failure(String),
}

impl Rollback {
    pub(super) fn silent(cause: impl Into<String>) -> Self {
        Self::replies(Vec::new(), cause)
    }

    pub(super) fn reply(message: ServerOpcodeMessage, cause: impl Into<String>) -> Self {
        Self::messages([message], cause)
    }

    pub(super) fn messages(
        messages: impl IntoIterator<Item = ServerOpcodeMessage>,
        cause: impl Into<String>,
    ) -> Self {
        match messages
            .into_iter()
            .map(lyracore_protocol::world::encode)
            .collect()
        {
            Ok(replies) => Self::replies(replies, cause),
            Err(error) => Self::Failure(error.to_string()),
        }
    }

    pub(super) fn replies(replies: Vec<Packet>, cause: impl Into<String>) -> Self {
        Self::Refusal {
            replies,
            cause: cause.into(),
        }
    }

    pub(super) fn into_reducer_error(self) -> String {
        match self {
            Self::Refusal { replies, cause } => {
                spacetimedb::log::debug!("native request refused: {cause}");
                envelope(&replies)
            }
            Self::Failure(cause) => cause,
        }
    }
}

impl From<String> for Rollback {
    fn from(cause: String) -> Self {
        Self::Failure(cause)
    }
}

impl From<&str> for Rollback {
    fn from(cause: &str) -> Self {
        Self::Failure(cause.into())
    }
}

impl From<Refusal> for Rollback {
    fn from(refusal: Refusal) -> Self {
        Self::silent(refusal)
    }
}

impl From<ActionRefusal> for Rollback {
    fn from(refusal: ActionRefusal) -> Self {
        Self::silent(refusal)
    }
}

// The Host parses this as JSON `[[opcode, [byte, ...]], ...]`; keep it byte-identical.
fn envelope(replies: &[Packet]) -> String {
    let packets = replies
        .iter()
        .map(|packet| {
            let body = packet
                .body
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            format!("[{}, [{body}]]", packet.opcode)
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("native_reply:[{packets}]")
}

/// The refusal packets in a reducer error that `envelope` wrote.
pub(super) fn replies_in(error: &str) -> Option<Vec<(u16, Vec<u8>)>> {
    let list = error.strip_prefix("native_reply:")?;
    let mut packets: Vec<(u16, Vec<u8>)> = Vec::new();
    let (mut depth, mut number) = (0, None::<u32>);
    for character in list.chars() {
        if let Some(digit) = character.to_digit(10) {
            number = Some(number.unwrap_or(0).checked_mul(10)?.checked_add(digit)?);
            continue;
        }
        if let Some(value) = number.take() {
            match depth {
                2 => packets.push((u16::try_from(value).ok()?, Vec::new())),
                3 => packets.last_mut()?.1.push(u8::try_from(value).ok()?),
                _ => return None,
            }
        }
        match character {
            '[' => depth += 1,
            ']' => depth -= 1,
            ',' | ' ' => {}
            _ => return None,
        }
    }
    (depth == 0).then_some(packets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_round_trip_through_the_envelope() {
        let replies = vec![
            Packet {
                opcode: 0x3a,
                body: vec![0, 255, 7],
            },
            Packet {
                opcode: 0x1dd,
                body: Vec::new(),
            },
        ];
        assert_eq!(
            replies_in(&envelope(&replies)),
            Some(vec![(0x3a, vec![0, 255, 7]), (0x1dd, Vec::new())])
        );
        assert_eq!(replies_in("native_reply:[]"), Some(Vec::new()));
        assert_eq!(replies_in("plain failure"), None);
    }

    #[test]
    fn a_silent_refusal_is_an_empty_reply_list() {
        assert_eq!(
            Rollback::silent("gate").into_reducer_error(),
            "native_reply:[]"
        );
    }

    #[test]
    fn refusal_replies_keep_their_order_opcodes_and_bytes() {
        let replies = vec![
            Packet {
                opcode: 0x0130,
                body: vec![133, 0, 0, 0, 2, 0x17],
            },
            Packet {
                opcode: 0x01b6,
                body: Vec::new(),
            },
            Packet {
                opcode: 0x0005,
                body: vec![255],
            },
        ];
        assert_eq!(
            Rollback::replies(replies, "gate").into_reducer_error(),
            "native_reply:[[304, [133, 0, 0, 0, 2, 23]],[438, []],[5, [255]]]"
        );
    }

    #[test]
    fn a_failure_keeps_its_cause_without_an_envelope() {
        assert_eq!(
            Rollback::from("native World Session is no longer current").into_reducer_error(),
            "native World Session is no longer current"
        );
    }
}
