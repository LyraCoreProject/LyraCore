use spacetimedb::{log, Timestamp};

/// The most spells resolving inside each other's effects at once (CONTEXT.md "Trigger Depth").
pub(super) const TRIGGER_DEPTH_LIMIT: usize = 8;
/// The most spells one reducer resolves inside other spells' effects (CONTEXT.md "Trigger Depth").
pub(super) const NESTED_RESOLUTION_BUDGET: u32 = 1024;

#[derive(Default)]
struct ResolvingSpells {
    reducer: Option<Timestamp>,
    spell_ids: Vec<u32>,
    nested_resolutions: u32,
    depth_warned: bool,
    budget_warned: bool,
}

impl ResolvingSpells {
    fn in_reducer(&mut self, reducer: Timestamp) -> &mut Self {
        if self.reducer != Some(reducer) {
            *self = Self {
                reducer: Some(reducer),
                ..Self::default()
            };
        }
        self
    }

    fn admit(&mut self, spell_id: u32) -> Result<(), String> {
        if self.spell_ids.len() >= TRIGGER_DEPTH_LIMIT {
            if !self.depth_warned {
                self.depth_warned = true;
                log::warn!(
                    "spell {spell_id} refused past Trigger Depth {TRIGGER_DEPTH_LIMIT}: \
                     spells {:?} resolve inside each other's effects",
                    self.spell_ids
                );
            }
            return Err(format!(
                "spell {spell_id} is past Trigger Depth {TRIGGER_DEPTH_LIMIT}"
            ));
        }
        if !self.spell_ids.is_empty() && self.nested_resolutions >= NESTED_RESOLUTION_BUDGET {
            if !self.budget_warned {
                self.budget_warned = true;
                log::warn!(
                    "spell {spell_id} refused past {NESTED_RESOLUTION_BUDGET} nested resolutions \
                     in one reducer: spells {:?} resolve inside each other's effects",
                    self.spell_ids
                );
            }
            return Err(format!(
                "spell {spell_id} is past {NESTED_RESOLUTION_BUDGET} nested resolutions"
            ));
        }
        Ok(())
    }
}

thread_local! {
    // Keyed by reducer timestamp, so a trapped reducer's leftover links never count.
    static RESOLVING: std::cell::RefCell<ResolvingSpells> =
        std::cell::RefCell::new(ResolvingSpells::default());
}

/// One spell resolving on the call stack; dropping it ends the link.
pub(super) struct TriggerLink;

impl TriggerLink {
    /// Refuses the spell when it would resolve past the Trigger Depth or the nested budget.
    pub(super) fn enter(reducer: Timestamp, spell_id: u32) -> Result<Self, String> {
        RESOLVING.with_borrow_mut(|resolving| {
            let resolving = resolving.in_reducer(reducer);
            resolving.admit(spell_id)?;
            if !resolving.spell_ids.is_empty() {
                resolving.nested_resolutions += 1;
            }
            resolving.spell_ids.push(spell_id);
            Ok(Self)
        })
    }

    /// The refusal [`Self::enter`] would give now, without entering.
    pub(super) fn check(reducer: Timestamp, spell_id: u32) -> Result<(), String> {
        RESOLVING.with_borrow_mut(|resolving| resolving.in_reducer(reducer).admit(spell_id))
    }
}

impl Drop for TriggerLink {
    fn drop(&mut self) {
        RESOLVING.with_borrow_mut(|resolving| {
            resolving.spell_ids.pop();
            if resolving.spell_ids.is_empty() {
                resolving.depth_warned = false;
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
        let at = reducer(0);
        let mut links: Vec<TriggerLink> = (0..TRIGGER_DEPTH_LIMIT as u32)
            .map(|spell_id| TriggerLink::enter(at, spell_id).unwrap())
            .collect();
        assert!(TriggerLink::check(at, 99).is_err());
        assert!(TriggerLink::enter(at, 99).is_err());

        links.pop();
        assert!(TriggerLink::enter(at, 99).is_ok());
    }

    #[test]
    fn a_later_reducer_ignores_links_a_trapped_reducer_left_behind() {
        let trapped: Vec<TriggerLink> = (0..TRIGGER_DEPTH_LIMIT as u32)
            .map(|spell_id| TriggerLink::enter(reducer(10), spell_id).unwrap())
            .collect();
        std::mem::forget(trapped);

        assert!(TriggerLink::enter(reducer(11), 1).is_ok());
    }

    #[test]
    fn nested_resolutions_past_the_budget_are_refused_and_top_level_casts_stay_free() {
        let at = reducer(20);
        let root = TriggerLink::enter(at, 1).unwrap();
        for _ in 0..NESTED_RESOLUTION_BUDGET {
            drop(TriggerLink::enter(at, 2).unwrap());
        }
        assert!(TriggerLink::check(at, 2).is_err());
        assert!(TriggerLink::enter(at, 2).is_err());

        drop(root);
        assert!(TriggerLink::enter(at, 3).is_ok());
    }
}
