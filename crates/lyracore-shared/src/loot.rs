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
}

impl LootRefusal {
    pub const ALL: [Self; 7] = [
        Self::LootTagIneligible,
        Self::OutOfRange,
        Self::NoLootSource,
        Self::LooterUnavailable,
        Self::NothingToLoot,
        Self::RollUnavailable,
        Self::NotMasterLooter,
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
        }
    }

    pub fn parse_tag(tag: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|refusal| refusal.as_tag() == tag)
    }
}

#[cfg(test)]
mod tests {
    use super::LootRefusal;

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
}
