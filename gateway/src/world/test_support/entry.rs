//! World entry as a socket test reads it.

use super::*;

/// More frames than any world entry batch carries. Reaching it means the batch never ended.
const WORLD_ENTRY_FRAME_LIMIT: usize = 64;

/// Read one world entry batch: the login sequence, any item CREATEs and the self CREATE, up to and
/// including the zone's `SMSG_WEATHER`, which always closes the batch. A fresh entry and a
/// world-port re-entry end the same way. Panics when no `SMSG_WEATHER` arrives within
/// [`WORLD_ENTRY_FRAME_LIMIT`] frames.
pub(crate) fn drain_world_entry<S: Read>(
    client: &mut S,
    dec: &mut DecrypterHalf,
) -> Vec<ServerOpcodeMessage> {
    let mut frames = Vec::new();
    while frames.len() < WORLD_ENTRY_FRAME_LIMIT {
        let message = ServerOpcodeMessage::read_encrypted(&mut *client, dec)
            .unwrap_or_else(|e| panic!("world entry frame {} did not arrive: {e}", frames.len()));
        let ends_batch = matches!(message, ServerOpcodeMessage::SMSG_WEATHER(_));
        frames.push(message);
        if ends_batch {
            return frames;
        }
    }
    panic!("no SMSG_WEATHER within {WORLD_ENTRY_FRAME_LIMIT} frames: the world entry batch never ended");
}
