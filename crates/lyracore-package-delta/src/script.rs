//! The **Script Artifact**: the whole Runtime Scripts one Package ships, and the tracer that puts
//! several Packages' scripts together.
//!
//! A Package Delta states COLUMNS of rows the base import owns, so two Packages can each hold part
//! of one row and the tracer's job is to merge them. A Runtime Script has no base import behind it
//! and no other owner: the Package that ships a script ships all of it. So this artifact carries
//! WHOLE rows, and two Packages meeting on one row is never a merge — it is two Packages claiming
//! one identity, which only a human can settle.
//!
//! That difference is the whole reason this is a second artifact kind rather than more tables in
//! the claim schema. Everything else it shares: the envelope, the canonical bytes, the refuse-early
//! discipline, and the Import Family the applier runs it for.
//!
//! Nothing here writes anywhere or reads a clock. A [`ScriptArtifact`] that exists has already
//! passed every check the artifact promises.
//!
//! # The artifact
//!
//! ```json
//! {
//!   "kind": "script",
//!   "version": 1,
//!   "package": "example.sharper-fireballs",
//!   "source_hash": "0000000000000000000000000000000000000000000000000000000000000000",
//!   "scripts": [
//!     {
//!       "script_id": 100001,
//!       "name": "sharper-fireballs.greet",
//!       "event": "on_login",
//!       "priority": 0,
//!       "enabled": true,
//!       "source": "if event.actor then send_chat(event.actor, \"hello\") end"
//!     }
//!   ]
//! }
//! ```
//!
//! `kind` is what tells the two artifacts apart. A Package Delta shipped its version 1 without one,
//! so a file carrying NO `kind` is a Package Delta and a file carrying one states what it is —
//! see [`artifact_kind`], which reads that member alone so a router can pick a parser without
//! committing to it.
//!
//! # What it refuses
//!
//! Unknown versions and members, a script outside the Package script range, a name that is not a
//! name, an event that is neither the Module's nor this Package's, an empty source, and one Package
//! shipping two scripts at one identifier or under one name. All at [`ScriptArtifact::parse`],
//! before an applier sees the artifact.
//!
//! There is no `operation` member and no delete. A Package ships the set of scripts it has; an
//! apply reconciles the shard to exactly that set, so removing a script from a Datascript removes
//! it from the shard on the next apply with nothing to state and nothing to undo.
//!
//! # What it traces
//!
//! [`trace_scripts`] puts several Packages' artifacts together. Two Packages shipping one
//! `script_id`, or one name, is a [`ScriptConflict`] naming both Packages. There is no merge case
//! and there are no priority numbers to break the tie with: `priority` orders scripts at an event,
//! never Packages against each other.

use core::fmt;

use serde_json::Value;

use crate::canonical::write_json_string;
use crate::delta::{expect_members, member, object, string, PackageId, SourceHash};
use crate::error::{DeltaError, ScriptRefusal};
use crate::ids::is_package_script_id;

/// The Import Family the script artifact belongs to.
///
/// The same name `apply_package_deltas` takes and `game_package_import.family` records. Unlike
/// every other family it has no base import: no DBC and no dump holds a Runtime Script, so the
/// family's "reload" is applying the enabled plan and nothing else.
pub const SCRIPT_FAMILY: &str = "script";

/// The value the artifact's `kind` member carries.
pub const SCRIPT_ARTIFACT_KIND: &str = "script";

/// The Script Artifact version this build reads and writes.
pub const SCRIPT_VERSION: u64 = 1;

/// Every event a Runtime Script may bind to: the Module's `HOOK_EVENTS` catalogue, in its order.
///
/// This is the SECOND copy of that catalogue — the first is `HOOK_EVENTS` in `module/build.rs`,
/// which generates the dispatch. A pure crate cannot read the Module's build script, so the copy is
/// held here and the Module asserts the two are identical against the `GAME_HOOK_EVENT_NAMES` its
/// build emits. Adding an event without updating this list fails that test rather than silently
/// refusing every Package that binds to it.
pub const HOOK_EVENT_NAMES: &[&str] = &[
    "on_damage_taken",
    "on_death_prevented",
    "on_creature_spawn",
    "on_levelup",
    "on_group_invite",
    "on_death",
    "on_kill",
    "on_aggro",
    "on_cast_resolved",
    "on_loot",
    "on_quest_accept",
    "on_quest_turnin",
    "on_login",
    "on_logout",
    "on_gossip_select",
    "on_creature_death",
    "on_hp_threshold",
    "on_go_used",
];

/// Which artifact a file in a Package's generated directory holds.
///
/// One Package ships every kind it has into one directory, so a reader that walks `*.json` meets
/// all of them and has to route rather than parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactKind {
    /// A Package Delta: column claims on rows a base import owns. Carries no `kind` member,
    /// because version 1 of that artifact shipped before there was a second kind to tell it from.
    Delta,
    /// A Script Artifact: whole Runtime Script rows.
    Script,
    /// A kind this build does not read. Held as written so a refusal can quote it.
    Other(String),
}

impl fmt::Display for ArtifactKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Delta => f.write_str("a Package Delta"),
            Self::Script => f.write_str("a Script Artifact"),
            Self::Other(kind) => write!(f, "an artifact of unknown kind `{kind}`"),
        }
    }
}

/// Which artifact these bytes hold, from the root `kind` member alone.
///
/// Reads nothing else, so a router picks a parser without paying for a parse it may not want and
/// without deciding anything the real parser will decide better.
///
/// `None` when the bytes are not a JSON object with a string-or-absent `kind`: no parser in this
/// crate can read them, so a router hands them to the one it wanted and lets that parser report
/// exactly what is wrong with them.
#[must_use]
pub fn artifact_kind(json: &str) -> Option<ArtifactKind> {
    let root: Value = serde_json::from_str(json).ok()?;
    match root.as_object()?.get("kind") {
        None => Some(ArtifactKind::Delta),
        Some(Value::String(kind)) if kind == SCRIPT_ARTIFACT_KIND => Some(ArtifactKind::Script),
        Some(Value::String(kind)) => Some(ArtifactKind::Other(kind.clone())),
        Some(_) => None,
    }
}

/// The unique, human-readable name of one Runtime Script.
///
/// Same character rules as a [`PackageId`], and for the same reason: the name reaches a diagnostic,
/// a conflict report and an operator's SQL, and a name differing from another only in case would
/// look like one script in all three. Convention prefixes it with the shipping Package, but nothing
/// enforces that — two Packages colliding on a name is a refusal that says so, which is more useful
/// than a naming rule that makes the collision unspellable and the mistake invisible.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScriptName(String);

impl ScriptName {
    /// Accepts 1 to 64 characters of `a-z`, `0-9`, `-`, `_` or `.`.
    ///
    /// # Errors
    /// [`DeltaError::InvalidScriptName`] for anything else.
    pub fn new(name: impl Into<String>) -> Result<Self, DeltaError> {
        let name = name.into();
        let shaped = !name.is_empty()
            && name.len() <= 64
            && name.bytes().all(|b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_' | b'.')
            });
        if shaped {
            Ok(Self(name))
        } else {
            Err(DeltaError::Script(ScriptRefusal::InvalidName {
                found: name,
            }))
        }
    }

    /// The name as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ScriptName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The event a Runtime Script is bound to: one name from [`HOOK_EVENT_NAMES`], or a Package Event
/// the shipping Package fires itself.
///
/// A core event is a closed list rather than a free string, checked at the parse. The Module fires
/// a fixed set of them, so a script bound to a name outside it would be a script that silently
/// never runs — the single hardest failure for a package author to diagnose.
///
/// A **Package Event** is spelled `<package>.<name>`, where `<package>` is the artifact's own
/// Package identity and `<name>` is a lowercase letter followed by lowercase letters, digits or
/// `_`. The prefix is what
/// keeps the same guarantee for an open name: a Package may only bind events it fires itself, so
/// two Packages cannot reach each other's decisions and a typo cannot land on a name some other
/// Package happens to own.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EventBinding(String);

impl EventBinding {
    /// Resolves an `event` member against the catalogue, then against `package`'s own events.
    ///
    /// # Errors
    /// [`DeltaError::Script`] with [`ScriptRefusal::UnknownEvent`], which names every event this
    /// build fires and the shape of a Package Event the artifact's own Package may bind.
    pub fn parse(name: &str, package: &PackageId) -> Result<Self, DeltaError> {
        let known = HOOK_EVENT_NAMES.contains(&name)
            || name
                .strip_prefix(package.as_str())
                .and_then(|rest| rest.strip_prefix('.'))
                .is_some_and(is_package_event_name);
        if known {
            Ok(Self(name.to_owned()))
        } else {
            Err(DeltaError::Script(ScriptRefusal::UnknownEvent {
                found: name.to_owned(),
                package: package.as_str().to_owned(),
            }))
        }
    }

    /// The event name, as the artifact writes it and the Module's dispatch spells it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EventBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The `<name>` half of a Package Event: a lowercase letter, then lowercase letters, digits or `_`.
///
/// Narrower than a [`ScriptName`] on purpose. No `.`, so the Package prefix stays the only dot and
/// the split between "whose event" and "which event" can never be ambiguous; no `-`, so the name
/// reads the same in Lua, in a Rust identifier and in `spacetime sql`.
fn is_package_event_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_lowercase())
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// One whole Runtime Script, exactly as it will sit in `game_script`.
///
/// Every member is authored. The content hash the table also carries is NOT here: it is derived
/// from `source` by the applier, so it cannot disagree with the source it describes.
// `script_id` repeats the type name because it is the DURABLE column name, shared with
// `game_script.script_id` and with `game_spell_effect.script_id` that will point at it. A shorter
// field here would make the artifact, the table and the accessor three different words for one
// thing.
#[allow(clippy::struct_field_names)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    script_id: u32,
    name: ScriptName,
    event: EventBinding,
    priority: i32,
    enabled: bool,
    source: String,
}

impl Script {
    /// Builds a script, refusing everything the artifact promises it will refuse.
    ///
    /// # Errors
    /// [`DeltaError::ScriptIdNotInPackageBand`] for an identifier a Package may not ship at, and
    /// [`DeltaError::EmptyScriptSource`] for a script with no Lua in it.
    pub fn new(
        script_id: u32,
        name: ScriptName,
        event: EventBinding,
        priority: i32,
        enabled: bool,
        source: impl Into<String>,
    ) -> Result<Self, DeltaError> {
        if !is_package_script_id(script_id) {
            return Err(DeltaError::Script(ScriptRefusal::IdNotInPackageBand {
                script_id,
            }));
        }
        let source = source.into();
        if source.trim().is_empty() {
            return Err(DeltaError::Script(ScriptRefusal::EmptySource {
                name: name.0.clone(),
            }));
        }
        Ok(Self {
            script_id,
            name,
            event,
            priority,
            enabled,
            source,
        })
    }

    /// The identifier this script keeps on every Shard.
    #[must_use]
    pub const fn script_id(&self) -> u32 {
        self.script_id
    }

    /// The unique name.
    #[must_use]
    pub const fn name(&self) -> &ScriptName {
        &self.name
    }

    /// The event this script runs for.
    #[must_use]
    pub const fn event(&self) -> &EventBinding {
        &self.event
    }

    /// Where this script sits among the scripts bound to its event: lower runs first, and
    /// `script_id` breaks a tie.
    #[must_use]
    pub const fn priority(&self) -> i32 {
        self.priority
    }

    /// Whether the Package ships this script switched on. A disabled script is still applied to the
    /// shard — it is a fact about the Package — but no event invokes it.
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    /// The Lua the Runtime Script Host runs.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }
}

/// One Package's whole set of Runtime Scripts: its identity, the source it was generated from, and
/// every script it ships.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptArtifact {
    package: PackageId,
    source_hash: SourceHash,
    scripts: Vec<Script>,
}

impl ScriptArtifact {
    /// Builds an artifact and puts its scripts in canonical order.
    ///
    /// # Errors
    /// [`DeltaError::DuplicateScriptId`] or [`DeltaError::DuplicateScriptName`] when one Package
    /// ships two scripts at one identity. Both are checked here rather than at the tracer, because
    /// a Package disagreeing with ITSELF is a Datascript defect, not a decision between Packages.
    pub fn new(
        package: PackageId,
        source_hash: SourceHash,
        mut scripts: Vec<Script>,
    ) -> Result<Self, DeltaError> {
        scripts.sort_by_key(Script::script_id);
        for pair in scripts.windows(2) {
            if pair[0].script_id == pair[1].script_id {
                return Err(DeltaError::Script(ScriptRefusal::DuplicateId {
                    script_id: pair[0].script_id,
                }));
            }
        }

        let mut by_name: Vec<&ScriptName> = scripts.iter().map(Script::name).collect();
        by_name.sort();
        for pair in by_name.windows(2) {
            if pair[0] == pair[1] {
                return Err(DeltaError::Script(ScriptRefusal::DuplicateName {
                    name: pair[0].0.clone(),
                }));
            }
        }

        Ok(Self {
            package,
            source_hash,
            scripts,
        })
    }

    /// Reads an artifact from untrusted JSON.
    ///
    /// Every refusal happens here, before any caller could act on a script.
    ///
    /// # Errors
    /// [`DeltaError::WrongArtifactKind`] when the bytes are a Package Delta or a kind this build
    /// does not read, and any other [`DeltaError`] the members raise. The parse stops at the first
    /// problem.
    pub fn parse(json: &str) -> Result<Self, DeltaError> {
        let root: Value =
            serde_json::from_str(json).map_err(|e| DeltaError::Malformed(e.to_string()))?;
        let root = object(&root, "")?;

        // Before the closed member list, not after: a Package Delta read by this parser would
        // otherwise be reported as an unexpected `claims` member, which describes the symptom
        // rather than the mistake. What kind these bytes are is the first question.
        match root.get("kind") {
            Some(Value::String(kind)) if kind == SCRIPT_ARTIFACT_KIND => {}
            Some(Value::String(kind)) => {
                return Err(DeltaError::WrongArtifactKind {
                    expected: "a Script Artifact",
                    found: ArtifactKind::Other(kind.clone()).to_string(),
                })
            }
            Some(_) => {
                return Err(DeltaError::WrongJsonType {
                    path: "kind".to_owned(),
                    expected: "a string",
                })
            }
            None => {
                return Err(DeltaError::WrongArtifactKind {
                    expected: "a Script Artifact",
                    found: ArtifactKind::Delta.to_string(),
                })
            }
        }

        expect_members(
            root,
            "",
            &["kind", "package", "scripts", "source_hash", "version"],
        )?;

        let version = member(root, "", "version")?;
        let version = version.as_u64().ok_or(DeltaError::WrongJsonType {
            path: "version".to_owned(),
            expected: "a version number",
        })?;
        if version != SCRIPT_VERSION {
            return Err(DeltaError::UnsupportedVersion {
                found: version.to_string(),
            });
        }

        let package = PackageId::new(string(member(root, "", "package")?, "package")?)?;
        let source_hash =
            SourceHash::new(string(member(root, "", "source_hash")?, "source_hash")?)?;

        let scripts = member(root, "", "scripts")?
            .as_array()
            .ok_or(DeltaError::WrongJsonType {
                path: "scripts".to_owned(),
                expected: "an array of scripts",
            })?;
        let scripts = scripts
            .iter()
            .enumerate()
            .map(|(index, script)| parse_script(script, index, &package))
            .collect::<Result<Vec<_>, _>>()?;

        Self::new(package, source_hash, scripts)
    }

    /// Writes the artifact back as canonical JSON.
    ///
    /// Byte-stable on the same terms as a Package Delta's canonical form: no whitespace, members in
    /// a fixed declared order, scripts sorted by identifier, and strings escaping only what JSON
    /// requires. Two artifacts that say the same thing produce the same bytes.
    #[must_use]
    pub fn to_canonical_json(&self) -> String {
        let mut out = String::new();
        out.push_str("{\"kind\":");
        write_json_string(&mut out, SCRIPT_ARTIFACT_KIND);
        out.push_str(",\"version\":");
        out.push_str(&SCRIPT_VERSION.to_string());
        out.push_str(",\"package\":");
        write_json_string(&mut out, self.package.as_str());
        out.push_str(",\"source_hash\":");
        write_json_string(&mut out, self.source_hash.as_str());
        out.push_str(",\"scripts\":[");
        for (index, script) in self.scripts.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            out.push_str("{\"script_id\":");
            out.push_str(&script.script_id.to_string());
            out.push_str(",\"name\":");
            write_json_string(&mut out, script.name.as_str());
            out.push_str(",\"event\":");
            write_json_string(&mut out, script.event.as_str());
            out.push_str(",\"priority\":");
            out.push_str(&script.priority.to_string());
            out.push_str(",\"enabled\":");
            out.push_str(if script.enabled { "true" } else { "false" });
            out.push_str(",\"source\":");
            write_json_string(&mut out, &script.source);
            out.push('}');
        }
        out.push_str("]}");
        out
    }

    /// The Package this artifact belongs to.
    #[must_use]
    pub const fn package(&self) -> &PackageId {
        &self.package
    }

    /// The digest of the Datascript source this artifact was generated from.
    #[must_use]
    pub const fn source_hash(&self) -> &SourceHash {
        &self.source_hash
    }

    /// The scripts, in canonical (identifier) order.
    #[must_use]
    pub fn scripts(&self) -> &[Script] {
        &self.scripts
    }
}

fn parse_script(value: &Value, index: usize, package: &PackageId) -> Result<Script, DeltaError> {
    let path = format!("scripts[{index}]");
    let script = object(value, &path)?;
    expect_members(
        script,
        &path,
        &[
            "enabled",
            "event",
            "name",
            "priority",
            "script_id",
            "source",
        ],
    )?;

    let script_id = member(script, &path, "script_id")?;
    let script_id = script_id
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or(DeltaError::WrongJsonType {
            path: format!("{path}.script_id"),
            expected: "a whole number in 0..=4294967295",
        })?;

    let name = ScriptName::new(string(
        member(script, &path, "name")?,
        &format!("{path}.name"),
    )?)?;
    let event = EventBinding::parse(
        string(member(script, &path, "event")?, &format!("{path}.event"))?,
        package,
    )?;

    let priority = member(script, &path, "priority")?;
    let priority = priority
        .as_i64()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or(DeltaError::WrongJsonType {
            path: format!("{path}.priority"),
            expected: "a whole number in -2147483648..=2147483647",
        })?;

    let enabled = member(script, &path, "enabled")?
        .as_bool()
        .ok_or(DeltaError::WrongJsonType {
            path: format!("{path}.enabled"),
            expected: "a boolean",
        })?;

    let source = string(member(script, &path, "source")?, &format!("{path}.source"))?;

    Script::new(script_id, name, event, priority, enabled, source)
}

// ===========================================================================================
//  The tracer — several Packages' scripts, together.
// ===========================================================================================

/// One script in a merged plan, and the Package that ships it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TracedScript {
    /// The whole script row.
    pub script: Script,
    /// The Package that ships it, for provenance and for a conflict report.
    pub package: PackageId,
    /// That Package's source digest, carried onto the row so a Shard records which revision of
    /// which Package put the script there.
    pub source_hash: SourceHash,
}

/// Two Packages claiming one Runtime Script identity. Only a human can settle it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptConflict {
    /// Both Packages ship a script at this identifier.
    Id {
        /// The contested identifier.
        script_id: u32,
        /// The Package that shipped it first in the plan.
        first: PackageId,
        /// The Package that shipped it second.
        second: PackageId,
    },
    /// Both Packages ship a script under this name.
    Name {
        /// The contested name.
        name: ScriptName,
        /// The Package that shipped it first in the plan.
        first: PackageId,
        /// The Package that shipped it second.
        second: PackageId,
    },
}

impl fmt::Display for ScriptConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Id {
                script_id,
                first,
                second,
            } => write!(
                f,
                "script {script_id} is shipped by both `{first}` and `{second}`"
            ),
            Self::Name {
                name,
                first,
                second,
            } => write!(
                f,
                "Runtime Script name `{name}` is shipped by both `{first}` and `{second}`"
            ),
        }
    }
}

/// Every Package's scripts merged, and every disagreement between them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScriptTrace {
    scripts: Vec<TracedScript>,
    conflicts: Vec<ScriptConflict>,
}

impl ScriptTrace {
    /// The merged scripts, ordered by identifier so one set of Packages always produces one plan.
    #[must_use]
    pub fn scripts(&self) -> &[TracedScript] {
        &self.scripts
    }

    /// Every disagreement, in the order they were found. Reported in full rather than one at a
    /// time: the operator fixing them wants the whole list in one pass.
    #[must_use]
    pub fn conflicts(&self) -> &[ScriptConflict] {
        &self.conflicts
    }

    /// True when no two Packages disagree, which is the only state an applier may write.
    #[must_use]
    pub fn is_clear(&self) -> bool {
        self.conflicts.is_empty()
    }
}

/// Merge several Packages' Script Artifacts into one plan, reporting every collision.
///
/// A script is never merged with another: the whole row belongs to one Package. So the only thing
/// to trace is identity — an identifier or a name that two Packages both claim.
///
/// A conflicting script is still carried into [`ScriptTrace::scripts`], from whichever Package
/// shipped it first. That keeps the plan describable in a report; it is not applyable, because an
/// applier refuses a plan that is not clear.
#[must_use]
pub fn trace_scripts(artifacts: &[ScriptArtifact]) -> ScriptTrace {
    let mut trace = ScriptTrace::default();
    for artifact in artifacts {
        for script in &artifact.scripts {
            if let Some(seen) = trace
                .scripts
                .iter()
                .find(|traced| traced.script.script_id == script.script_id)
            {
                trace.conflicts.push(ScriptConflict::Id {
                    script_id: script.script_id,
                    first: seen.package.clone(),
                    second: artifact.package.clone(),
                });
                continue;
            }
            if let Some(seen) = trace
                .scripts
                .iter()
                .find(|traced| traced.script.name == script.name)
            {
                trace.conflicts.push(ScriptConflict::Name {
                    name: script.name.clone(),
                    first: seen.package.clone(),
                    second: artifact.package.clone(),
                });
                continue;
            }
            trace.scripts.push(TracedScript {
                script: script.clone(),
                package: artifact.package.clone(),
                source_hash: artifact.source_hash.clone(),
            });
        }
    }
    trace.scripts.sort_by_key(|traced| traced.script.script_id);
    trace
}
