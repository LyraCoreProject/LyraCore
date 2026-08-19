//! EventAI edge dispatch.

use super::{EventAiRequest, EventContext, EventKind};

crate::game_hook!(on_aggro, fn creature_ai_on_aggro(ctx, payload) {
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u64;
    super::evaluate_context(
        ctx,
        EventAiRequest::Edge(EventContext {
            kind: EventKind::OnAggro,
            creature_guid: payload.creature_guid,
            invoker_guid: Some(payload.target_guid),
            event_target_guid: Some(payload.target_guid),
            current_target_guid: Some(payload.target_guid),
            assisted: payload.assist,
            now_ms,
        }),
    );
});
