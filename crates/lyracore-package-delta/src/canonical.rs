//! The canonical form of a Package Delta.
//!
//! Two artifacts that say the same thing must produce the same bytes, so that a source hash, a diff
//! and a signature all mean what they look like they mean. The writer is hand-built rather than
//! derived, because the byte stability is this crate's promise and must not move under a library's
//! formatting default.
//!
//! The rules, all of them:
//!
//!  * No whitespace anywhere, and no trailing newline.
//!  * Object members appear in a fixed declared order; `fields` members appear sorted by name.
//!  * Claims appear sorted by table, then spell, then effect index.
//!  * Integers are plain decimal. An integer column written as `100.0` or `1e2` is refused at the
//!    parse rather than rounded here, so there is only ever one spelling to write.
//!  * An unsigned 64-bit value is a decimal string with no sign, no padding and no separators.
//!  * A float is the shortest decimal that reads back as the same `f32`, always with a decimal
//!    point, so `1`, `1.0` and `1.00` all become `1.0`.
//!  * A string escapes only what JSON requires, using the short escape where one exists.

use core::fmt;

use crate::delta::{Claim, PackageDelta, PrimaryKey, DELTA_VERSION};
use crate::schema::FieldValue;

pub(crate) fn write_delta(delta: &PackageDelta) -> String {
    let mut out = String::new();
    out.push_str("{\"version\":");
    out.push_str(&DELTA_VERSION.to_string());
    out.push_str(",\"package\":");
    write_json_string(&mut out, delta.package().as_str());
    out.push_str(",\"source_hash\":");
    write_json_string(&mut out, delta.source_hash().as_str());
    out.push_str(",\"claims\":[");
    for (index, claim) in delta.claims().iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        write_claim(&mut out, claim);
    }
    out.push_str("]}");
    out
}

fn write_claim(out: &mut String, claim: &Claim) {
    out.push_str("{\"table\":");
    write_json_string(out, claim.table().as_str());
    out.push_str(",\"key\":");
    write_key(out, claim.key());
    out.push_str(",\"operation\":");
    write_json_string(out, claim.operation().as_str());
    out.push_str(",\"fields\":{");
    for (index, (name, value)) in claim.fields().iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        write_json_string(out, name);
        out.push(':');
        write_value(out, value);
    }
    out.push_str("}}");
}

fn write_key(out: &mut String, key: PrimaryKey) {
    match key {
        PrimaryKey::Spell { spell_id } => {
            out.push_str("{\"spell_id\":");
            out.push_str(&spell_id.to_string());
            out.push('}');
        }
        PrimaryKey::SpellEffect {
            spell_id,
            effect_index,
        } => {
            out.push_str("{\"spell_id\":");
            out.push_str(&spell_id.to_string());
            out.push_str(",\"effect_index\":");
            out.push_str(&effect_index.to_string());
            out.push('}');
        }
        PrimaryKey::Item { entry } => {
            out.push_str("{\"entry\":");
            out.push_str(&entry.to_string());
            out.push('}');
        }
    }
}

fn write_value(out: &mut String, value: &FieldValue) {
    out.push_str("{\"type\":");
    write_json_string(out, value.field_type().as_str());
    out.push_str(",\"value\":");
    out.push_str(&scalar_literal(value));
    out.push('}');
}

/// One claimed value in its canonical JSON spelling, without the surrounding type tag. Shared with
/// `FieldValue`'s `Display`, so a conflict report quotes a value exactly as the artifact writes it.
pub(crate) fn scalar_literal(value: &FieldValue) -> String {
    let mut out = String::new();
    match value {
        FieldValue::U8(n) => out.push_str(&n.to_string()),
        FieldValue::U16(n) => out.push_str(&n.to_string()),
        FieldValue::U32(n) => out.push_str(&n.to_string()),
        FieldValue::U64(n) => {
            out.push('"');
            out.push_str(&n.to_string());
            out.push('"');
        }
        FieldValue::I32(n) => out.push_str(&n.to_string()),
        FieldValue::F32(n) => write_f32(&mut out, *n),
        FieldValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        FieldValue::Str(s) => write_json_string(&mut out, s),
    }
    out
}

/// `f32` `Display` already gives the shortest decimal that reads back as the same value and never
/// uses exponent form. The only thing missing is a decimal point on a whole number, which keeps a
/// float column visually and textually distinct from an integer one.
fn write_f32(out: &mut String, value: f32) {
    let text = value.to_string();
    out.push_str(&text);
    if !text.contains('.') {
        out.push_str(".0");
    }
}

pub(crate) fn write_json_string(out: &mut String, text: &str) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                use fmt::Write as _;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}
