//! `lyracore-delta-check <artifact.json>...` — parse a set of generated Package artifacts, trace
//! them together, and print the plan they form.
//!
//! Two artifact kinds arrive, because a Package ships every kind it has into one generated
//! directory: Package Deltas and Script Artifacts. Each file is routed on its own `kind` member and
//! traced with its own kind, and both traces have to come out clear.
//!
//! This is the check a Datascript author gets after a build, before anything reaches a shard. It
//! needs no client data, no database and no realm: it reads the named files and nothing else, which
//! is why it can run on any machine in under a second.
//!
//! Exit status is the whole answer: 0 when every artifact parses and the Packages agree, 1 when an
//! artifact is refused, two Packages claim the same column of one row, or two Packages claim one
//! Runtime Script identity. The output names the file and the exact claim or script in every case,
//! because the reader is the author who has to fix one line of a Datascript or one script file.
//!
//! Trace ALL of a realm's enabled artifacts in one invocation. Conflicts exist BETWEEN Packages, so
//! checking one file at a time can only ever prove that one file parses.
//!
//! `lyracore-delta-check --print-events` is a second, unrelated mode: it prints the Event Binding
//! catalogue and exits, reading no file and tracing nothing. See [`print_events`].

use std::path::Path;
use std::process::ExitCode;

use lyracore_package_delta::{
    artifact_kind, trace, trace_scripts, ArtifactKind, ClaimTrace, Operation, PackageDelta,
    ScriptArtifact, ScriptTrace, HOOK_EVENT_NAMES,
};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--print-events") {
        return print_events(&args);
    }

    let paths = args;
    if paths.is_empty() || paths.iter().any(|a| a == "-h" || a == "--help") {
        eprintln!(
            "usage: lyracore-delta-check <artifact.json>...\n\n\
             Parses each generated Package artifact — a Package Delta or a Script Artifact — \
             traces each kind together, and prints the plan.\n\
             Exits non-zero on a refused artifact, a Claim Conflict or a Runtime Script \
             collision. Reads no client data and no database.\n\n\
             Name every enabled Package's artifacts in ONE run: a conflict is between \
             Packages, so one file alone cannot show one.\n\n\
             lyracore-delta-check --print-events prints the Event Binding catalogue instead."
        );
        return ExitCode::FAILURE;
    }

    let mut deltas: Vec<PackageDelta> = Vec::new();
    let mut delta_paths: Vec<&String> = Vec::new();
    let mut scripts: Vec<ScriptArtifact> = Vec::new();
    let mut script_paths: Vec<&String> = Vec::new();
    for path in &paths {
        match read_artifact(Path::new(path)) {
            Ok(Artifact::Delta(delta)) => {
                deltas.push(delta);
                delta_paths.push(path);
            }
            Ok(Artifact::Script(artifact)) => {
                scripts.push(artifact);
                script_paths.push(path);
            }
            Err(refusal) => {
                eprintln!("{refusal}");
                return ExitCode::FAILURE;
            }
        }
    }

    let claim_trace = trace(&deltas);
    let script_trace = trace_scripts(&scripts);
    print!(
        "{}",
        report(
            &delta_paths,
            &deltas,
            &claim_trace,
            &script_paths,
            &scripts,
            &script_trace
        )
    );

    if claim_trace.is_clear() && script_trace.is_clear() {
        eprintln!(
            "{} artifact(s) check out over {} row(s) and {} Runtime Script(s).",
            deltas.len() + scripts.len(),
            claim_trace.rows().len(),
            script_trace.scripts().len()
        );
        return ExitCode::SUCCESS;
    }
    if !claim_trace.is_clear() {
        eprintln!(
            "{} claim conflict(s) between the named Packages — a human chooses.",
            claim_trace.conflicts().len()
        );
    }
    if !script_trace.is_clear() {
        eprintln!(
            "{} Runtime Script collision(s) between the named Packages — a human chooses.",
            script_trace.conflicts().len()
        );
    }
    ExitCode::FAILURE
}

/// One generated artifact, already routed to the parser its own `kind` chose.
enum Artifact {
    Delta(PackageDelta),
    Script(ScriptArtifact),
}

/// Prints [`HOOK_EVENT_NAMES`], one event per line, in catalogue order, and exits.
///
/// `--print-events` must be the only argument: this mode reads no artifact, no other file and the
/// clock never enters it, so the output is byte-stable across machines and runs. A future Package's
/// TypeScript authoring surface is generated from this output, so it stays the one place outside the
/// Module's own build that knows the Event Binding catalogue — `HOOK_EVENT_NAMES` here is already a
/// second copy of `module/build.rs`'s `HOOK_EVENTS`; this prints that copy rather than hand-writing
/// a third.
fn print_events(args: &[String]) -> ExitCode {
    if args.len() != 1 {
        eprintln!(
            "usage: lyracore-delta-check --print-events\n\n\
             Prints the catalogue of events a Runtime Script may bind to, one per line, in \
             catalogue order, and exits. Takes no other argument."
        );
        return ExitCode::FAILURE;
    }

    for event in HOOK_EVENT_NAMES {
        println!("{event}");
    }
    ExitCode::SUCCESS
}

/// Read one file and hand it to the parser its own `kind` names.
///
/// Routing on `kind` rather than on the file name: a Package chooses its own artifact file names,
/// and the member inside the file is the only thing that says what it holds. A kind this build does
/// not read reaches [`PackageDelta::parse`], which refuses it by name.
///
/// A refusal names the FILE, so an author with several Packages open knows which one to fix.
fn read_artifact(path: &Path) -> Result<Artifact, String> {
    let json = std::fs::read_to_string(path)
        .map_err(|e| format!("{}: cannot read: {e}", path.display()))?;
    let refuse = |e| format!("{}: {e}", path.display());
    match artifact_kind(&json) {
        Some(ArtifactKind::Script) => ScriptArtifact::parse(&json).map(Artifact::Script),
        _ => PackageDelta::parse(&json).map(Artifact::Delta),
    }
    .map_err(refuse)
}

/// The plan: every Package with its file and row counts, then every claimed row, then every
/// conflict. The same shape the importer's Package Delta stage prints, so an author and an operator
/// read the same report.
fn report(
    delta_paths: &[&String],
    deltas: &[PackageDelta],
    traced: &ClaimTrace,
    script_paths: &[&String],
    scripts: &[ScriptArtifact],
    script_trace: &ScriptTrace,
) -> String {
    let mut out = String::from("\n=== Package Deltas ===\n");
    for (path, delta) in delta_paths.iter().zip(deltas) {
        let counts = delta.claim_counts();
        out.push_str(&format!(
            "  {:<32} {:>3} updated  {:>3} spells  {:>3} effects   {path}\n",
            delta.package(),
            counts.updated_rows,
            counts.inserted_spells,
            counts.inserted_effects,
        ));
    }

    for row in traced.rows() {
        let operation = match row.operation() {
            Operation::Insert => "insert",
            Operation::Update => "update",
        };
        let fields: Vec<String> = row
            .fields()
            .iter()
            .map(|(name, claimed)| format!("{name}={} [{}]", claimed.value, claimed.claimed_by))
            .collect();
        out.push_str(&format!(
            "  {operation} {} {} — {}\n",
            row.table(),
            row.key(),
            fields.join(", ")
        ));
    }

    for conflict in traced.conflicts() {
        out.push_str(&format!("  CONFLICT {conflict}\n"));
    }

    out.push_str("=== Runtime Scripts ===\n");
    for (path, artifact) in script_paths.iter().zip(scripts) {
        out.push_str(&format!(
            "  {:<32} {:>3} script(s)   {path}\n",
            artifact.package(),
            artifact.scripts().len()
        ));
    }
    for traced in script_trace.scripts() {
        let script = &traced.script;
        out.push_str(&format!(
            "  {:>6} {:<32} {} priority {}{}\n",
            script.script_id(),
            script.name(),
            script.event(),
            script.priority(),
            if script.enabled() { "" } else { " (disabled)" },
        ));
    }
    for conflict in script_trace.conflicts() {
        out.push_str(&format!("  COLLISION {conflict}\n"));
    }
    out.push_str("======================\n\n");
    out
}
