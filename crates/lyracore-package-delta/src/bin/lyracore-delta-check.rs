//! `lyracore-delta-check <artifact.json>...` — parse a set of Package Delta artifacts, trace them
//! together, and print the plan they form.
//!
//! This is the check a Datascript author gets after a build, before anything reaches a shard. It
//! needs no client data, no database and no realm: it reads the named files and nothing else, which
//! is why it can run on any machine in under a second.
//!
//! Exit status is the whole answer: 0 when every artifact parses and the claims merge cleanly,
//! 1 when an artifact is refused or two Packages claim the same column of one row. The output names
//! the file and the exact claim in both cases, because the reader is the author who has to fix one
//! line of a Datascript.
//!
//! Trace ALL of a realm's enabled artifacts in one invocation. Conflicts exist BETWEEN Packages, so
//! checking one file at a time can only ever prove that one file parses.

use std::path::Path;
use std::process::ExitCode;

use lyracore_package_delta::{trace, ClaimTrace, Operation, PackageDelta};

fn main() -> ExitCode {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() || paths.iter().any(|a| a == "-h" || a == "--help") {
        eprintln!(
            "usage: lyracore-delta-check <artifact.json>...\n\n\
             Parses each Package Delta artifact, traces them together, and prints the plan.\n\
             Exits non-zero on a refused artifact or a Claim Conflict. Reads no client data and \
             no database.\n\n\
             Name every enabled Package's artifacts in ONE run: a Claim Conflict is between \
             Packages, so one file alone cannot show one."
        );
        return ExitCode::FAILURE;
    }

    let mut deltas = Vec::with_capacity(paths.len());
    for path in &paths {
        match read_artifact(Path::new(path)) {
            Ok(delta) => deltas.push(delta),
            Err(refusal) => {
                eprintln!("{refusal}");
                return ExitCode::FAILURE;
            }
        }
    }

    let traced = trace(&deltas);
    print!("{}", report(&paths, &deltas, &traced));

    if traced.is_clear() {
        eprintln!(
            "{} artifact(s) check out over {} row(s).",
            deltas.len(),
            traced.rows().len()
        );
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "{} claim conflict(s) between the named Packages — a human chooses.",
            traced.conflicts().len()
        );
        ExitCode::FAILURE
    }
}

/// A refusal names the FILE, so an author with several Packages open knows which one to fix.
fn read_artifact(path: &Path) -> Result<PackageDelta, String> {
    let json = std::fs::read_to_string(path)
        .map_err(|e| format!("{}: cannot read: {e}", path.display()))?;
    PackageDelta::parse(&json).map_err(|e| format!("{}: {e}", path.display()))
}

/// The plan: every Package with its file and row counts, then every claimed row, then every
/// conflict. The same shape the importer's Package Delta stage prints, so an author and an operator
/// read the same report.
fn report(paths: &[String], deltas: &[PackageDelta], traced: &ClaimTrace) -> String {
    let mut out = String::from("\n=== Package Deltas ===\n");
    for (path, delta) in paths.iter().zip(deltas) {
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
    out.push_str("======================\n\n");
    out
}
