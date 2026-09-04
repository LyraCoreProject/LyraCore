//! The loot tier contract shared by the Module and the Gateway.

/// Why the Module refused a loot Durable Request. The tag is the whole reducer error text, so
/// neither tier matches on human prose. The Module keeps the detail in its own log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LootRefusal {
    /// The Actor is not in the Loot Source's Loot Tag eligibility set.
    LootTagIneligible,
    /// The Loot Source is on another map, in another instance, or beyond loot range.
    OutOfRange,
    /// The named Loot Source does not exist, or is not a lootable creature corpse.
    NoLootSource,
    /// The looter is not in the world, or is dead.
    LooterUnavailable,
    /// The Loot Source has nothing left in the requested slot or purse.
    NothingToLoot,
    /// No open Loot Roll answers this vote, or the voter already voted.
    RollUnavailable,
    /// The Actor does not hold the master-looter right on that row.
    NotMasterLooter,
    /// The master-loot recipient is no longer in the world.
    RecipientUnavailable,
    /// The master-loot recipient has no room for the item.
    RecipientInventoryFull,
}

impl LootRefusal {
    pub const ALL: [Self; 9] = [
        Self::LootTagIneligible,
        Self::OutOfRange,
        Self::NoLootSource,
        Self::LooterUnavailable,
        Self::NothingToLoot,
        Self::RollUnavailable,
        Self::NotMasterLooter,
        Self::RecipientUnavailable,
        Self::RecipientInventoryFull,
    ];

    pub fn as_tag(self) -> &'static str {
        match self {
            Self::LootTagIneligible => "loot:loot_tag_ineligible",
            Self::OutOfRange => "loot:out_of_range",
            Self::NoLootSource => "loot:no_loot_source",
            Self::LooterUnavailable => "loot:looter_unavailable",
            Self::NothingToLoot => "loot:nothing_to_loot",
            Self::RollUnavailable => "loot:roll_unavailable",
            Self::NotMasterLooter => "loot:not_master_looter",
            Self::RecipientUnavailable => "loot:recipient_unavailable",
            Self::RecipientInventoryFull => "loot:recipient_inventory_full",
        }
    }

    pub fn parse_tag(tag: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|refusal| refusal.as_tag() == tag)
    }
}

/// A failure in the trusted Gateway entry before a loot core can answer the request. These tags
/// cross the reducer boundary so legacy untagged gameplay compatibility cannot hide a broken
/// Operator or Actor invariant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LootBoundaryFailure {
    OperatorRejected,
    MissingActor,
}

impl LootBoundaryFailure {
    pub const ALL: [Self; 2] = [Self::OperatorRejected, Self::MissingActor];

    pub fn as_tag(self) -> &'static str {
        match self {
            Self::OperatorRejected => "loot:boundary_operator_rejected",
            Self::MissingActor => "loot:boundary_missing_actor",
        }
    }

    pub fn parse_tag(tag: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|failure| failure.as_tag() == tag)
    }
}

#[cfg(test)]
mod tests {
    use super::{LootBoundaryFailure, LootRefusal};

    #[test]
    fn every_refusal_tag_round_trips() {
        for refusal in LootRefusal::ALL {
            assert_eq!(LootRefusal::parse_tag(refusal.as_tag()), Some(refusal));
        }
        assert_eq!(LootRefusal::parse_tag("loot:"), None);
        assert_eq!(
            LootRefusal::parse_tag("loot_tag_ineligible: actor_guid=7 corpse_guid=11"),
            None
        );
        assert_eq!(
            LootRefusal::parse_tag("gw_take_loot reducer timed out after 10s"),
            None
        );
    }

    #[test]
    fn every_boundary_failure_tag_round_trips() {
        for failure in LootBoundaryFailure::ALL {
            assert_eq!(
                LootBoundaryFailure::parse_tag(failure.as_tag()),
                Some(failure)
            );
            assert_eq!(LootRefusal::parse_tag(failure.as_tag()), None);
        }
    }
}
