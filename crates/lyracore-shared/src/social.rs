#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContactRefusal {
    ActorUnavailable,
    AddSelf,
    AlreadyOnList,
    ListFull,
    NotOnList,
    Enemy,
}

impl ContactRefusal {
    pub fn as_tag(self) -> &'static str {
        match self {
            Self::ActorUnavailable => "social:actor_unavailable",
            Self::AddSelf => "social:add_self",
            Self::AlreadyOnList => "social:already_on_list",
            Self::ListFull => "social:list_full",
            Self::NotOnList => "social:not_on_list",
            Self::Enemy => "social:enemy",
        }
    }
}
