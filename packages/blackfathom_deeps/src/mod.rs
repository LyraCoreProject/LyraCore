use crate::encounter::{self, EncounterSignal, ENCOUNTER_DONE};

crate::encounter_package!(BlackfathomDeepsKelris, fn kelris(ctx, instance_id, signal) {
    match signal {
        EncounterSignal::Complete => {
            encounter::set_encounter_state(ctx, instance_id, 1, ENCOUNTER_DONE)
        }
        other => Err(format!("Kelris does not accept encounter signal {other:?}")),
    }
});
