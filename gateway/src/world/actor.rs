//! The Character a Durable Request acts as.

use std::num::NonZeroU64;

/// A resolved Actor guid. Guid 0 means the World Session has no Character yet, so it is never an
/// Actor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Actor(NonZeroU64);

impl Actor {
    pub(crate) fn new(guid: u64) -> Option<Self> {
        NonZeroU64::new(guid).map(Self)
    }

    pub(crate) fn guid(self) -> u64 {
        self.0.get()
    }
}

#[cfg(test)]
mod tests {
    use super::Actor;

    #[test]
    fn guid_zero_is_no_actor() {
        assert_eq!(Actor::new(0), None);
        assert_eq!(Actor::new(7).map(Actor::guid), Some(7));
    }
}
