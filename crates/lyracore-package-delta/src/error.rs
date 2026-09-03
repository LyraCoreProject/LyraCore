//! Why a Package artifact was refused.
//!
//! Every refusal here happens before an applier sees the artifact, so no partial write is possible.
//! Each variant names the smallest thing that was wrong, because the reader is a package author who
//! has to fix one line of a Datascript.
//!
//! A refusal about an identifier band or a key shape is family-specific and says so in its name
//! (`SpellIdNotClientSafe`, `EffectIndexOutOfRange`, `ScriptIdNotInPackageBand`). An Import Family
//! adds its own variants rather than widening one, so no message loses the detail an author needs
//! to fix the claim. The envelope refusals above them — malformed JSON, an unknown member, a bad
//! Package identity — are shared, because every artifact kind carries the same envelope.

use core::fmt;

use crate::ids::{
    FIXTURE_CREATURE_ID_CEIL, FIXTURE_CREATURE_ID_FLOOR, FIXTURE_RESERVED_ID_CEIL,
    FIXTURE_RESERVED_ID_FLOOR, FIXTURE_SPELL_ID_CEIL, FIXTURE_SPELL_ID_FLOOR,
    MAX_QUEST_OBJECTIVE_INDEX, MAX_QUEST_REWARD_CHOICE_INDEX, MAX_SPELL_EFFECT_INDEX,
    MAX_STATS_LEVEL, PACKAGE_CAST_ID_CEIL, PACKAGE_CAST_ID_FLOOR, PACKAGE_CREATURE_ID_CEIL,
    PACKAGE_CREATURE_ID_FLOOR, PACKAGE_GAMEOBJECT_ID_CEIL, PACKAGE_GAMEOBJECT_ID_FLOOR,
    PACKAGE_GLOBALS_ID_CEIL, PACKAGE_GLOBALS_ID_FLOOR, PACKAGE_GOSSIP_ID_CEIL,
    PACKAGE_GOSSIP_ID_FLOOR, PACKAGE_ITEM_ID_CEIL, PACKAGE_ITEM_ID_FLOOR, PACKAGE_LOOT_ID_CEIL,
    PACKAGE_LOOT_ID_FLOOR, PACKAGE_QUEST_ID_CEIL, PACKAGE_QUEST_ID_FLOOR, PACKAGE_SCRIPT_ID_CEIL,
    PACKAGE_SCRIPT_ID_FLOOR, PACKAGE_SPELLMETA_ID_CEIL, PACKAGE_SPELLMETA_ID_FLOOR,
    PACKAGE_SPELL_ID_CEIL, PACKAGE_SPELL_ID_FLOOR, PACKAGE_TRAINER_ID_CEIL,
    PACKAGE_TRAINER_ID_FLOOR,
};
use crate::schema::{FieldType, Table};
use crate::script::HOOK_EVENT_NAMES;

/// A Package artifact that cannot be trusted, and the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeltaError {
    /// The bytes are not JSON at all.
    Malformed(String),
    /// The bytes hold a different artifact kind than the parser that read them.
    ///
    /// One Package's generated directory holds every kind it ships, side by side, so a router that
    /// hands the wrong file to the wrong parser must be told exactly that rather than shown a
    /// member-by-member complaint about an artifact that was never meant for it.
    WrongArtifactKind {
        /// What the parser reads.
        expected: &'static str,
        /// What the bytes actually are.
        found: String,
    },
    /// The artifact declares a version this build does not implement.
    UnsupportedVersion {
        /// The declared version.
        found: String,
    },
    /// A required member is absent. `path` is a dotted location inside the artifact.
    MissingMember {
        /// Where the member belonged.
        path: String,
    },
    /// A member this build does not know. Refused rather than ignored, so a typo in a Datascript
    /// never becomes a silently dropped claim.
    UnexpectedMember {
        /// Where the member appeared.
        path: String,
        /// The member's name.
        member: String,
    },
    /// A member holds the wrong JSON shape.
    WrongJsonType {
        /// Where the member appeared.
        path: String,
        /// What was expected there.
        expected: &'static str,
    },
    /// The `package` member is not a usable Package identity.
    InvalidPackageId {
        /// The rejected value.
        found: String,
    },
    /// The `source_hash` member is not a 64-character lowercase hexadecimal digest.
    InvalidSourceHash {
        /// The rejected value.
        found: String,
    },
    /// The `table` member names a table outside the closed list.
    UnknownTable {
        /// The rejected name.
        found: String,
    },
    /// The `operation` member names an operation outside the closed list.
    UnknownOperation {
        /// The rejected name.
        found: String,
    },
    /// A claim asked for row deletion. Packages override a row to inert instead; there is no
    /// pre-image ledger that could undo a delete on replay.
    DeleteNotSupported,
    /// The `key` member does not describe a row of its table.
    MalformedKey {
        /// The table the key was meant for.
        table: Table,
        /// What is wrong with it.
        detail: String,
    },
    /// An inserted spell sits outside the range a Package may invent.
    SpellIdNotClientSafe {
        /// The rejected identifier.
        spell_id: u32,
    },
    /// The claim targets a seeded fixture row.
    SpellIdFixtureReserved {
        /// The rejected identifier.
        spell_id: u32,
    },
    /// The effect index names a slot no spell has.
    EffectIndexOutOfRange {
        /// The rejected index.
        effect_index: u8,
    },
    /// An inserted item sits outside the range a Package may invent.
    ItemIdNotClientSafe {
        /// The rejected identifier.
        entry: u32,
    },
    /// The claim targets a seeded fixture row.
    ItemIdFixtureReserved {
        /// The rejected identifier.
        entry: u32,
    },
    /// An inserted quest sits outside the range a Package may invent.
    QuestIdNotClientSafe {
        /// The rejected identifier.
        entry: u32,
    },
    /// The claim targets a seeded fixture row.
    QuestIdFixtureReserved {
        /// The rejected identifier.
        entry: u32,
    },
    /// The objective index names a slot no quest has.
    QuestObjectiveIndexOutOfRange {
        /// The rejected index.
        obj_index: u8,
    },
    /// The reward-choice index names a slot no quest has.
    QuestRewardChoiceIndexOutOfRange {
        /// The rejected index.
        choice_index: u8,
    },
    /// An inserted loot row sits outside the range a Package may invent.
    LootIdNotClientSafe {
        /// The rejected identifier.
        id: u64,
    },
    /// The claim targets a seeded fixture row.
    LootIdFixtureReserved {
        /// The rejected identifier.
        id: u64,
    },
    /// An inserted `game_creature_spell` row sits outside the range a Package may invent.
    CastIdNotClientSafe {
        /// The rejected identifier.
        id: u64,
    },
    /// The claim targets a seeded fixture row.
    CastIdFixtureReserved {
        /// The rejected identifier.
        id: u64,
    },
    /// An inserted `game_trainer_spell` row sits outside the range a Package may invent.
    TrainerIdNotClientSafe {
        /// The rejected identifier.
        id: u64,
    },
    /// The claim targets a seeded fixture row.
    TrainerIdFixtureReserved {
        /// The rejected identifier.
        id: u64,
    },
    /// An inserted gossip row sits outside the range a Package may invent.
    GossipIdNotClientSafe {
        /// The rejected identifier.
        id: u64,
    },
    /// The claim targets a seeded fixture row.
    GossipIdFixtureReserved {
        /// The rejected identifier.
        id: u64,
    },
    /// An inserted globals row sits outside the range a Package may invent.
    GlobalsIdNotClientSafe {
        /// The rejected identifier.
        id: u64,
    },
    /// The claim targets a seeded fixture row.
    GlobalsIdFixtureReserved {
        /// The rejected identifier.
        id: u64,
    },
    /// An inserted `game_spell_learn` row sits outside the range a Package may invent.
    SpellmetaIdNotClientSafe {
        /// The rejected identifier.
        id: u64,
    },
    /// The claim targets a seeded fixture row.
    SpellmetaIdFixtureReserved {
        /// The rejected identifier.
        id: u64,
    },
    /// An inserted creature template or creature spawn sits outside the range a Package may
    /// invent.
    CreatureIdNotClientSafe {
        /// The rejected identifier.
        id: u32,
    },
    /// The claim targets a seeded fixture row.
    CreatureIdFixtureReserved {
        /// The rejected identifier.
        id: u32,
    },
    /// An inserted gameobject template, trap or spawn sits outside the range a Package may invent.
    GameobjectIdNotClientSafe {
        /// The rejected identifier.
        id: u32,
    },
    /// The claim targets a seeded fixture row.
    GameobjectIdFixtureReserved {
        /// The rejected identifier.
        id: u32,
    },
    /// The stat-curve key names a character level no vanilla realm reaches.
    StatsLevelOutOfRange {
        /// The rejected level.
        level: u8,
    },
    /// An `insert` named a table that permits no inserts at all: its primary key names an entity
    /// another Import Family owns, which no Package may invent. `game_creature_cast`
    /// ([`Table::CreatureCast`]) is the worked example.
    InsertNotSupported {
        /// The table that refused the insert.
        table: Table,
    },
    /// A claim names a column the table does not have.
    UnknownField {
        /// The claimed table.
        table: Table,
        /// The rejected column name.
        field: String,
    },
    /// A value declares a type tag outside the closed list.
    UnknownFieldType {
        /// The claimed column.
        field: String,
        /// The rejected type tag.
        found: String,
    },
    /// A value declares a type the column does not take.
    FieldTypeMismatch {
        /// The claimed table.
        table: Table,
        /// The claimed column.
        field: String,
        /// The column's type.
        expected: FieldType,
        /// The declared type.
        found: FieldType,
    },
    /// A value does not fit the type it declares.
    ValueOutOfRange {
        /// The claimed column.
        field: String,
        /// The declared type.
        ty: FieldType,
        /// The value as written.
        literal: String,
    },
    /// A claim names a primary-key column in `fields`. The key already carries it.
    KeyColumnClaimed {
        /// The claimed table.
        table: Table,
        /// The primary-key column that was claimed.
        field: String,
    },
    /// An insert does not carry the whole row.
    IncompleteInsert {
        /// The claimed table.
        table: Table,
        /// The absent columns, in declaration order.
        missing: Vec<String>,
    },
    /// An update claims no columns, so it states no change.
    EmptyUpdate {
        /// The claimed table.
        table: Table,
    },
    /// One Package Delta claims the same row twice. A Package names each row once and lists every
    /// column it sets there.
    DuplicateClaim {
        /// The claimed table.
        table: Table,
        /// The row, as the conflict report writes it.
        key: String,
    },

    /// A refusal only the script family can raise.
    ///
    /// Grouped rather than flattened in beside the claim refusals: the script family shares this
    /// enum's envelope variants — malformed JSON, an unknown member, a bad Package identity — and
    /// nothing else, so its own refusals read better as one named group than as six more arms a
    /// claim reader has to skip past. A family with refusals of its own follows this shape.
    Script(ScriptRefusal),
}

/// Why a Script Artifact was refused, for the refusals no other Import Family can raise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptRefusal {
    /// A shipped Runtime Script sits outside the range a Package may ship in.
    IdNotInPackageBand {
        /// The rejected identifier.
        script_id: u32,
    },
    /// The `name` member is not a usable Runtime Script name.
    InvalidName {
        /// The rejected value.
        found: String,
    },
    /// The `event` member names neither a Module hook event nor a Package Event of the artifact's
    /// own Package.
    UnknownEvent {
        /// The rejected name.
        found: String,
        /// The artifact's own Package, which is the only Package whose events it may bind.
        package: String,
    },
    /// A Runtime Script ships no Lua at all. An empty script is a Datascript that emitted nothing,
    /// not a script that does nothing.
    EmptySource {
        /// The script that carries no source.
        name: String,
    },
    /// One Script Artifact ships two scripts at one identifier.
    DuplicateId {
        /// The repeated identifier.
        script_id: u32,
    },
    /// One Script Artifact ships two scripts under one name.
    DuplicateName {
        /// The repeated name.
        name: String,
    },
}

impl fmt::Display for ScriptRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IdNotInPackageBand { script_id } => write!(
                f,
                "script {script_id} is outside the Package script range \
                 {PACKAGE_SCRIPT_ID_FLOOR}..={PACKAGE_SCRIPT_ID_CEIL}; a shipped Runtime Script \
                 must use an identifier the range clears for Packages"
            ),
            Self::InvalidName { found } => write!(
                f,
                "`{found}` is not a Runtime Script name: expected 1 to 64 characters of \
                 a-z, 0-9, `-`, `_` or `.`"
            ),
            Self::UnknownEvent { found, package } => write!(
                f,
                "unknown event `{found}`; a Runtime Script binds to one of {}, or to a Package \
                 Event of its own Package spelled `{package}.<name>`, where `<name>` is a \
                 lowercase letter followed by lowercase letters, digits or `_`",
                known_events()
            ),
            Self::EmptySource { name } => write!(
                f,
                "Runtime Script `{name}` carries no source; a script that should do nothing is \
                 shipped disabled, not empty"
            ),
            Self::DuplicateId { script_id } => write!(
                f,
                "script {script_id} is shipped twice by one Package; every Runtime Script needs \
                 its own identifier"
            ),
            Self::DuplicateName { name } => write!(
                f,
                "Runtime Script name `{name}` is shipped twice by one Package; every Runtime \
                 Script needs its own name"
            ),
        }
    }
}

impl fmt::Display for DeltaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SpellIdNotClientSafe { .. }
            | Self::SpellIdFixtureReserved { .. }
            | Self::EffectIndexOutOfRange { .. }
            | Self::ItemIdNotClientSafe { .. }
            | Self::ItemIdFixtureReserved { .. }
            | Self::QuestIdNotClientSafe { .. }
            | Self::QuestIdFixtureReserved { .. }
            | Self::QuestObjectiveIndexOutOfRange { .. }
            | Self::QuestRewardChoiceIndexOutOfRange { .. }
            | Self::LootIdNotClientSafe { .. }
            | Self::LootIdFixtureReserved { .. }
            | Self::CastIdNotClientSafe { .. }
            | Self::CastIdFixtureReserved { .. }
            | Self::TrainerIdNotClientSafe { .. }
            | Self::TrainerIdFixtureReserved { .. }
            | Self::GossipIdNotClientSafe { .. }
            | Self::GossipIdFixtureReserved { .. }
            | Self::GlobalsIdNotClientSafe { .. }
            | Self::GlobalsIdFixtureReserved { .. }
            | Self::SpellmetaIdNotClientSafe { .. }
            | Self::SpellmetaIdFixtureReserved { .. }
            | Self::CreatureIdNotClientSafe { .. }
            | Self::CreatureIdFixtureReserved { .. }
            | Self::GameobjectIdNotClientSafe { .. }
            | Self::GameobjectIdFixtureReserved { .. }
            | Self::StatsLevelOutOfRange { .. } => fmt_identifier_policy(self, f),
            // The script family groups its own refusals behind one variant, so it delegates as a
            // whole rather than adding six arms here.
            Self::Script(refusal) => refusal.fmt(f),
            other => fmt_general(other, f),
        }
    }
}

/// The identifier-band refusals, split out of [`DeltaError`]'s `Display` so neither half of the
/// message trips `clippy::too_many_lines` as a family's bands add their own variants. Split again
/// by kind for the same reason: "outside the band a Package may invent in" here, "reserved, or
/// outside the table's real domain" in [`fmt_reserved_or_out_of_range`].
fn fmt_identifier_policy(err: &DeltaError, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match err {
        DeltaError::SpellIdNotClientSafe { spell_id } => write!(
            f,
            "spell {spell_id} is outside the Package spell range \
             {PACKAGE_SPELL_ID_FLOOR}..={PACKAGE_SPELL_ID_CEIL}; an inserted spell must use an \
             identifier no client and no import can already own"
        ),
        DeltaError::ItemIdNotClientSafe { entry } => write!(
            f,
            "item {entry} is outside the Package item range \
             {PACKAGE_ITEM_ID_FLOOR}..={PACKAGE_ITEM_ID_CEIL}; an inserted item must use an \
             identifier no client and no import can already own"
        ),
        DeltaError::QuestIdNotClientSafe { entry } => write!(
            f,
            "quest {entry} is outside the Package quest range \
             {PACKAGE_QUEST_ID_FLOOR}..={PACKAGE_QUEST_ID_CEIL}; an inserted quest must use an \
             identifier no client and no import can already own"
        ),
        DeltaError::LootIdNotClientSafe { id } => write!(
            f,
            "loot row {id} is outside the Package loot range \
             {PACKAGE_LOOT_ID_FLOOR}..={PACKAGE_LOOT_ID_CEIL}; an inserted loot row must use an \
             identifier no client and no import can already own"
        ),
        DeltaError::CastIdNotClientSafe { id } => write!(
            f,
            "cast row {id} is outside the Package cast range \
             {PACKAGE_CAST_ID_FLOOR}..={PACKAGE_CAST_ID_CEIL}; an inserted cast row must use an \
             identifier no client and no import can already own"
        ),
        DeltaError::TrainerIdNotClientSafe { id } => write!(
            f,
            "trainer row {id} is outside the Package trainer range \
             {PACKAGE_TRAINER_ID_FLOOR}..={PACKAGE_TRAINER_ID_CEIL}; an inserted trainer row must \
             use an identifier no client and no import can already own"
        ),
        DeltaError::GossipIdNotClientSafe { id } => write!(
            f,
            "gossip row {id} is outside the Package gossip range \
             {PACKAGE_GOSSIP_ID_FLOOR}..={PACKAGE_GOSSIP_ID_CEIL}; an inserted gossip row must use \
             an identifier no client and no import can already own"
        ),
        DeltaError::GlobalsIdNotClientSafe { id } => write!(
            f,
            "globals row {id} is outside the Package globals range \
             {PACKAGE_GLOBALS_ID_FLOOR}..={PACKAGE_GLOBALS_ID_CEIL}; an inserted globals row must \
             use an identifier no client and no import can already own"
        ),
        DeltaError::SpellmetaIdNotClientSafe { id } => write!(
            f,
            "spell metadata row {id} is outside the Package spell metadata range \
             {PACKAGE_SPELLMETA_ID_FLOOR}..={PACKAGE_SPELLMETA_ID_CEIL}; an inserted spell \
             metadata row must use an identifier no client and no import can already own"
        ),
        DeltaError::CreatureIdNotClientSafe { id } => write!(
            f,
            "creature row {id} is outside the Package creature range \
             {PACKAGE_CREATURE_ID_FLOOR}..={PACKAGE_CREATURE_ID_CEIL}; an inserted creature \
             template or spawn must use an identifier no client and no import can already own"
        ),
        DeltaError::GameobjectIdNotClientSafe { id } => write!(
            f,
            "gameobject row {id} is outside the Package gameobject range \
             {PACKAGE_GAMEOBJECT_ID_FLOOR}..={PACKAGE_GAMEOBJECT_ID_CEIL}; an inserted gameobject \
             template, trap or spawn must use an identifier no client and no import can already own"
        ),
        other => fmt_reserved_or_out_of_range(other, f),
    }
}

/// The other half of the identifier policy: an identifier the fixtures own, and an index outside
/// the real domain of the table it names.
fn fmt_reserved_or_out_of_range(err: &DeltaError, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match err {
        DeltaError::SpellIdFixtureReserved { spell_id } => write!(
            f,
            "spell {spell_id} is fixture-reserved \
             ({FIXTURE_SPELL_ID_FLOOR}..={FIXTURE_SPELL_ID_CEIL} and \
             {FIXTURE_RESERVED_ID_FLOOR}..={FIXTURE_RESERVED_ID_CEIL}); no Package may claim it"
        ),
        DeltaError::EffectIndexOutOfRange { effect_index } => write!(
            f,
            "effect index {effect_index} is out of range; a spell has effects \
             0..={MAX_SPELL_EFFECT_INDEX}"
        ),
        DeltaError::ItemIdFixtureReserved { entry } => write!(
            f,
            "item {entry} is fixture-reserved \
             ({FIXTURE_RESERVED_ID_FLOOR}..={FIXTURE_RESERVED_ID_CEIL}); no Package may claim it"
        ),
        DeltaError::QuestIdFixtureReserved { entry } => write!(
            f,
            "quest {entry} is fixture-reserved \
             ({FIXTURE_RESERVED_ID_FLOOR}..={FIXTURE_RESERVED_ID_CEIL}); no Package may claim it"
        ),
        DeltaError::QuestObjectiveIndexOutOfRange { obj_index } => write!(
            f,
            "objective index {obj_index} is out of range; a quest has objectives \
             0..={MAX_QUEST_OBJECTIVE_INDEX}"
        ),
        DeltaError::QuestRewardChoiceIndexOutOfRange { choice_index } => write!(
            f,
            "reward choice index {choice_index} is out of range; a quest has reward choices \
             0..={MAX_QUEST_REWARD_CHOICE_INDEX}"
        ),
        DeltaError::LootIdFixtureReserved { id } => write!(
            f,
            "loot row {id} is fixture-reserved \
             ({FIXTURE_RESERVED_ID_FLOOR}..={FIXTURE_RESERVED_ID_CEIL}); no Package may claim it"
        ),
        DeltaError::CastIdFixtureReserved { id } => write!(
            f,
            "cast row {id} is fixture-reserved \
             ({FIXTURE_RESERVED_ID_FLOOR}..={FIXTURE_RESERVED_ID_CEIL}); no Package may claim it"
        ),
        DeltaError::TrainerIdFixtureReserved { id } => write!(
            f,
            "trainer row {id} is fixture-reserved \
             ({FIXTURE_RESERVED_ID_FLOOR}..={FIXTURE_RESERVED_ID_CEIL}); no Package may claim it"
        ),
        DeltaError::GossipIdFixtureReserved { id } => write!(
            f,
            "gossip row {id} is fixture-reserved \
             ({FIXTURE_RESERVED_ID_FLOOR}..={FIXTURE_RESERVED_ID_CEIL}); no Package may claim it"
        ),
        DeltaError::GlobalsIdFixtureReserved { id } => write!(
            f,
            "globals row {id} is fixture-reserved \
             ({FIXTURE_RESERVED_ID_FLOOR}..={FIXTURE_RESERVED_ID_CEIL}); no Package may claim it"
        ),
        DeltaError::SpellmetaIdFixtureReserved { id } => write!(
            f,
            "spell metadata row {id} is fixture-reserved \
             ({FIXTURE_RESERVED_ID_FLOOR}..={FIXTURE_RESERVED_ID_CEIL}); no Package may claim it"
        ),
        DeltaError::CreatureIdFixtureReserved { id } => write!(
            f,
            "creature row {id} is fixture-reserved \
             ({FIXTURE_CREATURE_ID_FLOOR}..={FIXTURE_CREATURE_ID_CEIL} for a template entry, \
             {FIXTURE_RESERVED_ID_FLOOR}..={FIXTURE_RESERVED_ID_CEIL} project-wide); no Package \
             may claim it"
        ),
        DeltaError::GameobjectIdFixtureReserved { id } => write!(
            f,
            "gameobject row {id} is fixture-reserved \
             ({FIXTURE_RESERVED_ID_FLOOR}..={FIXTURE_RESERVED_ID_CEIL}); no Package may claim it"
        ),
        DeltaError::StatsLevelOutOfRange { level } => write!(
            f,
            "level {level} is outside 1..={MAX_STATS_LEVEL}; no stat curve carries a row for it"
        ),
        other => unreachable!("{other:?} is not an identifier-policy refusal"),
    }
}

/// Every refusal that is not an identifier-band policy.
fn fmt_general(err: &DeltaError, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match err {
        DeltaError::Malformed(detail) => write!(f, "not valid JSON: {detail}"),
        DeltaError::WrongArtifactKind { expected, found } => write!(
            f,
            "these bytes are {found}, not {expected}; one Package ships each artifact kind in \
             its own file and each is read by its own parser"
        ),
        DeltaError::UnsupportedVersion { found } => write!(
            f,
            "unsupported Package Delta version {found}; this build implements version {}",
            crate::DELTA_VERSION
        ),
        DeltaError::MissingMember { path } => write!(f, "missing member `{path}`"),
        DeltaError::UnexpectedMember { path, member } => {
            write!(f, "unexpected member `{member}` in `{path}`")
        }
        DeltaError::WrongJsonType { path, expected } => {
            write!(f, "member `{path}` must be {expected}")
        }
        DeltaError::InvalidPackageId { found } => write!(
            f,
            "`{found}` is not a Package identity: expected 1 to 64 characters of \
             a-z, 0-9, `-`, `_` or `.`"
        ),
        DeltaError::InvalidSourceHash { found } => write!(
            f,
            "`{found}` is not a source hash: expected 64 lowercase hexadecimal characters"
        ),
        DeltaError::UnknownTable { found } => write!(
            f,
            "unknown table `{found}`; a Package Delta claims {}",
            known_tables()
        ),
        DeltaError::UnknownOperation { found } => {
            write!(
                f,
                "unknown operation `{found}`; expected `insert` or `update`"
            )
        }
        DeltaError::DeleteNotSupported => f.write_str(
            "row deletion is not supported; override the row to inert with an `update` instead",
        ),
        DeltaError::InsertNotSupported { table } => write!(
            f,
            "`{table}` rows cannot be inserted: the primary key names an entity another Import \
             Family owns, which no Package may invent; an existing row may still be updated"
        ),
        DeltaError::MalformedKey { table, detail } => {
            write!(f, "malformed `{table}` key: {detail}")
        }
        DeltaError::UnknownField { table, field } => {
            write!(f, "`{table}` has no claimable column `{field}`")
        }
        DeltaError::UnknownFieldType { field, found } => {
            write!(f, "column `{field}` declares unknown type `{found}`")
        }
        DeltaError::FieldTypeMismatch {
            table,
            field,
            expected,
            found,
        } => write!(
            f,
            "`{table}`.`{field}` is `{expected}`, but the claim declares `{found}`"
        ),
        DeltaError::ValueOutOfRange { field, ty, literal } => {
            write!(f, "column `{field}`: `{literal}` does not fit `{ty}`")
        }
        DeltaError::KeyColumnClaimed { table, field } => write!(
            f,
            "`{table}`.`{field}` is part of the primary key; the claim's `key` already names it"
        ),
        DeltaError::IncompleteInsert { table, missing } => write!(
            f,
            "an `insert` into `{table}` must carry the whole row; {} column(s) absent: {}",
            missing.len(),
            missing.join(", ")
        ),
        DeltaError::EmptyUpdate { table } => {
            write!(f, "an `update` on `{table}` must claim at least one column")
        }
        DeltaError::DuplicateClaim { table, key } => write!(
            f,
            "`{table}` row {key} is claimed twice by one Package; merge the two claims"
        ),
        other => unreachable!("{other:?} is an identifier-policy refusal, not a general one"),
    }
}

impl std::error::Error for DeltaError {}

/// The whole catalogue as a prose list — "`a`", "`a` or `b`", "`a`, `b` or `c`" — so an Import
/// Family that adds tables extends the refusal without touching the message.
fn known_tables() -> String {
    prose_list(
        Table::ALL.iter().map(|table| format!("`{table}`")),
        "no table at all in this build",
    )
}

/// The hook catalogue as the same prose list, so an event added to the Module extends this refusal
/// without touching the message.
fn known_events() -> String {
    prose_list(
        HOOK_EVENT_NAMES.iter().map(|event| format!("`{event}`")),
        "no event at all in this build",
    )
}

fn prose_list(items: impl Iterator<Item = String>, empty: &str) -> String {
    let quoted: Vec<String> = items.collect();
    match quoted.split_last() {
        None => empty.to_owned(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
    }
}
