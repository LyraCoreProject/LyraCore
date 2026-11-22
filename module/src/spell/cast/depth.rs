use spacetimedb::{log, Timestamp};

/// The most spells resolving inside each other's effects at once (CONTEXT.md "Trigger Depth").
pub(super) const TRIGGER_DEPTH_LIMIT: usize = 8;

#[derive(Default)]
struct ResolvingSpells {
    reducer: Option<Timestamp>,
    spell_ids: Vec<u32>,
    warned: bool,
}

thread_local! {
    // Keyed by reducer timestamp, so a trapped reducer's leftover links never count.
    static RESOLVING: std::cell::RefCell<ResolvingSpells> =
        std::cell::RefCell::new(ResolvingSpells::default());
}

/// One spell resolving on the call stack; dropping it ends the spell's link.
pub(super) struct TriggerDepth;

impl TriggerDepth {
    /// Refuses the spell when the spells already resolving in this reducer reach the limit.
    pub(super) fn enter(reducer: Timestamp, spell_id: u32) -> Result<Self, String> {
        RESOLVING.with_borrow_mut(|resolving| {
            if resolving.reducer != Some(reducer) {
                *resolving = ResolvingSpells {
                    reducer: Some(reducer),
                    ..ResolvingSpells::default()
                };
            }
            if resolving.spell_ids.len() >= TRIGGER_DEPTH_LIMIT {
                if !resolving.warned {
                    resolving.warned = true;
                    log::warn!(
                        "spell {spell_id} refused past Trigger Depth {TRIGGER_DEPTH_LIMIT}: \
                         spells {:?} resolve inside each other's effects",
                        resolving.spell_ids
                    );
                }
                return Err(format!(
                    "spell {spell_id} is past Trigger Depth {TRIGGER_DEPTH_LIMIT}"
                ));
            }
            resolving.spell_ids.push(spell_id);
            Ok(Self)
        })
    }
}

impl Drop for TriggerDepth {
    fn drop(&mut self) {
        RESOLVING.with_borrow_mut(|resolving| {
            resolving.spell_ids.pop();
            if resolving.spell_ids.is_empty() {
                resolving.warned = false;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REDUCER_MICROS: i64 = 1_800_000_000_000_000;

    fn reducer(offset_micros: i64) -> Timestamp {
        Timestamp::from_micros_since_unix_epoch(REDUCER_MICROS + offset_micros)
    }

    #[test]
    fn the_spell_past_the_limit_is_refused_and_a_returned_link_frees_its_place() {
        let mut links: Vec<TriggerDepth> = (0..TRIGGER_DEPTH_LIMIT as u32)
            .map(|spell_id| TriggerDepth::enter(reducer(0), spell_id).unwrap())
            .collect();
        assert!(TriggerDepth::enter(reducer(0), 99).is_err());

        links.pop();
        assert!(TriggerDepth::enter(reducer(0), 99).is_ok());
    }

    #[test]
    fn a_later_reducer_ignores_links_a_trapped_reducer_left_behind() {
        let trapped: Vec<TriggerDepth> = (0..TRIGGER_DEPTH_LIMIT as u32)
            .map(|spell_id| TriggerDepth::enter(reducer(0), spell_id).unwrap())
            .collect();
        std::mem::forget(trapped);

        assert!(TriggerDepth::enter(reducer(1), 1).is_ok());
    }
}
