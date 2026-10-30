use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Violation {
    pub property: String,
    pub recipient: u64,
    pub guid: u64,
    pub field: Option<u16>,
    pub detail: String,
}
