const DEFINITION_REVISION_DOMAIN: &[u8] = b"lyracore-eventai-definition-v1";

/// The revision the Module checks on `{subject}@{revision}@{rules}` before it loads a definition.
#[must_use]
pub fn definition_revision(subject: &str, rules: &str) -> u64 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(DEFINITION_REVISION_DOMAIN);
    hasher.update(subject.as_bytes());
    hasher.update(b"@");
    hasher.update(rules.as_bytes());
    u64::from_le_bytes(
        hasher.finalize().as_bytes()[..8]
            .try_into()
            .expect("a BLAKE3 digest has at least eight bytes"),
    )
}

/// One `import_creature_ai_definitions` row; `subject` is `entry:{entry}` or `guid:{guid}`.
#[must_use]
pub fn encode_definition(subject: &str, rules: &str) -> String {
    format!("{subject}@{}@{rules}", definition_revision(subject, rules))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_definition_row_keeps_the_revision_the_importer_has_always_written() {
        assert_eq!(
            encode_definition("entry:6", "17,aggro,100,1,once,all,ordinary,flee"),
            "entry:6@12649877892465313675@17,aggro,100,1,once,all,ordinary,flee"
        );
    }
}
