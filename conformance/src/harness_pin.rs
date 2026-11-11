use anyhow::{ensure, Result};

/// The wire-harness release the conformance tools build against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HarnessPin<'a> {
    pub tag: &'a str,
    pub revision: &'a str,
}

impl<'a> HarnessPin<'a> {
    /// Reads `.wire-harness-rev`: one release tag and its full commit SHA; `#` starts a comment.
    pub fn parse(text: &'a str) -> Result<Self> {
        let fields: Vec<_> = text
            .lines()
            .map(|line| line.split('#').next().unwrap_or_default())
            .flat_map(str::split_whitespace)
            .collect();
        let [tag, revision] = fields.as_slice() else {
            anyhow::bail!("pin needs one tag and full SHA");
        };
        ensure!(tag.starts_with('v'), "pin must name a release tag");
        ensure!(
            revision.len() == 40 && revision.bytes().all(|b| b.is_ascii_hexdigit()),
            "pin needs a full SHA"
        );
        Ok(Self { tag, revision })
    }
}

pub fn harness_pin() -> HarnessPin<'static> {
    HarnessPin::parse(include_str!("../../.wire-harness-rev"))
        .expect("the committed .wire-harness-rev is a valid pin")
}
