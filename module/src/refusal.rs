/// A Gate's Refusal whose client reply does not depend on the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refusal(String);

impl From<String> for Refusal {
    fn from(cause: String) -> Self {
        Self(cause)
    }
}

impl From<&str> for Refusal {
    fn from(cause: &str) -> Self {
        Self(cause.into())
    }
}

impl From<Refusal> for String {
    fn from(refusal: Refusal) -> Self {
        refusal.0
    }
}

impl From<crate::actor::ActionRefusal> for Refusal {
    fn from(refusal: crate::actor::ActionRefusal) -> Self {
        Self(refusal.detail)
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
