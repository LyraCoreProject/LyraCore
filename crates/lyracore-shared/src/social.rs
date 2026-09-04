//! The Module↔Gateway CONTACT wire contract: why the Module refused a friends- or ignore-list
//! change. The party half of the social pane is [`crate::group`].

/// Why the Module refused a contact-list Durable Request. The tag is the whole reducer error text,
/// so neither tier matches on human prose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContactRefusal {
    /// The acting Character is temporarily unable to change its contact lists.
    ActorUnavailable,
    /// A Character may not befriend or ignore itself.
    AddSelf,
    /// No Character row holds the guid the Gateway resolved.
    NoSuchPlayer,
    /// The target is already on that same list.
    AlreadyOnList,
    /// That list is at its cap.
    ListFull,
    /// The remove named a target that is not on that list.
    NotOnList,
}

impl ContactRefusal {
    pub const ALL: [Self; 6] = [
        Self::ActorUnavailable,
        Self::AddSelf,
        Self::NoSuchPlayer,
        Self::AlreadyOnList,
        Self::ListFull,
        Self::NotOnList,
    ];

    pub fn as_tag(self) -> &'static str {
        match self {
            Self::ActorUnavailable => "social:actor_unavailable",
            Self::AddSelf => "social:add_self",
            Self::NoSuchPlayer => "social:no_such_player",
            Self::AlreadyOnList => "social:already_on_list",
            Self::ListFull => "social:list_full",
            Self::NotOnList => "social:not_on_list",
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
    use super::ContactRefusal;

    #[test]
    fn every_contact_refusal_tag_round_trips() {
        for refusal in ContactRefusal::ALL {
            assert_eq!(ContactRefusal::parse_tag(refusal.as_tag()), Some(refusal));
        }
        assert_eq!(ContactRefusal::parse_tag("social:"), None);
        assert_eq!(
            ContactRefusal::parse_tag("gw_add_friend reducer timed out after 10s"),
            None
        );
    }
}
