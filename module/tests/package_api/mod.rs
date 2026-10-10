//! The Package API lint: every crate-root path an installed Package names is on Package API
//! version 2, as `docs/package-api.md` documents it.

mod contract;
mod paths;
#[path = "../../build_support/source.rs"]
mod source;

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

pub use contract::Contract;
use contract::{Verdict, ROOTS};
pub use paths::UnsupportedSyntax;
use source::{collect_rs_files, file_gate, strip_source};

/// The comment that clears one out-of-surface path on its own line. It must carry a reason, so an
/// exemption is readable where it is used and greppable across a tree.
const PACKAGE_API_EXEMPT: &str = "// package-api: exempt";

#[derive(Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub package: String,
    /// The file as `packages/<package>/src/<path>`.
    pub file: PathBuf,
    pub line: usize,
    pub finding: Finding,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Finding {
    Outside {
        path: String,
        replacement: Option<&'static str>,
    },
    GatedChild {
        path: String,
        child: &'static str,
        gate: &'static str,
    },
    PackageModule {
        path: String,
    },
    Unsupported {
        kind: UnsupportedSyntax,
        written: String,
    },
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let package = &self.package;
        let at = format!("{}:{}", self.file.display(), self.line);
        match &self.finding {
            Finding::Outside { path, replacement } => {
                write!(
                    f,
                    "Package `{package}` names `{path}` at {at}, outside Package API version 2 \
                     (docs/package-api.md). "
                )?;
                match replacement {
                    Some(replacement) => write!(f, "Use `{replacement}`."),
                    None => write!(
                        f,
                        "Name a path under {}, or write `{PACKAGE_API_EXEMPT} <reason>` on that \
                         line and raise the gap with the maintainers.",
                        ROOTS
                            .iter()
                            .map(|root| format!("`crate::{root}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                }
            }
            Finding::GatedChild { path, child, gate } => write!(
                f,
                "Package `{package}` names `{path}` at {at}. `crate::package::{child}` is Package \
                 API only in a file whose first non-blank line is `{gate}`."
            ),
            Finding::PackageModule { path } => write!(
                f,
                "Package `{package}` names the `crate::package` module itself as `{path}` at {at}. \
                 Name `crate::package::<item>` instead, so `fixture` and `test` cannot leave their \
                 file gates through the module."
            ),
            Finding::Unsupported { kind, written } => {
                let instruction = match kind {
                    UnsupportedSyntax::WholeCrateAlias => {
                        "Whole-crate aliases can hide core paths across Rust scopes. Spell each \
                         core path as `crate::<Package API root>` instead."
                    }
                    UnsupportedSyntax::PathAttribute => {
                        "Path attributes break Package source discovery and module-depth checks. \
                         Use Rust's normal `mod.rs`, `<name>.rs`, or `<name>/mod.rs` layout \
                         instead."
                    }
                    UnsupportedSyntax::IncludeMacro => {
                        "`include!` can add Rust source that the Package API lint cannot \
                         discover. Put Package Rust in a normal `.rs` source file instead."
                    }
                };
                write!(
                    f,
                    "Package `{package}` uses unsupported syntax {written} at {at}. {instruction}"
                )
            }
        }
    }
}

/// Check every Rust file of every Package under `packages_dir`, a directory per Package, the
/// same Packages the build compiles: those with a `src/` directory.
pub fn check_inventory(contract: &Contract, packages_dir: &Path) -> Result<(), Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();
    if !packages_dir.is_dir() {
        return Ok(());
    }
    let mut packages: Vec<PathBuf> = fs::read_dir(packages_dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", packages_dir.display()))
        .map(|entry| entry.expect("readable Package entry").path())
        .filter(|package| package.join("src").is_dir())
        .collect();
    packages.sort();
    for package in packages {
        let name = package
            .file_name()
            .expect("a Package directory has a name")
            .to_string_lossy()
            .into_owned();
        let src = package.join("src");
        let mut files = Vec::new();
        collect_rs_files(&src, &mut files);
        files.sort();
        for file in files {
            let source = fs::read_to_string(&file)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", file.display()));
            let relative = file.strip_prefix(&src).expect("collected beneath src/");
            diagnostics.extend(check_source(contract, &name, relative, &source));
        }
    }
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(diagnostics)
    }
}

/// Every Package API finding in one Package file, in line order. `relative_path` is the file's
/// path beneath its Package's `src/`.
pub fn check_source(
    contract: &Contract,
    package: &str,
    relative_path: &Path,
    source: &str,
) -> Vec<Diagnostic> {
    let stripped = strip_source(source);
    let exempt_lines: Vec<usize> = stripped
        .line_comments
        .iter()
        .filter(|(_, comment)| {
            comment
                .split_once(PACKAGE_API_EXEMPT)
                .is_some_and(|(_, reason)| !reason.trim().is_empty())
        })
        .map(|(line, _)| *line)
        .collect();
    // `mod.rs` is its directory's module; any other file adds its stem as one module segment.
    let components = relative_path.components().count();
    let depth = if relative_path
        .file_name()
        .is_some_and(|name| name == "mod.rs")
    {
        components - 1
    } else {
        components
    };
    let gate = file_gate(source);
    let found = paths::file_paths(&stripped.code, depth);

    let mut findings: Vec<(usize, Finding)> = found
        .unsupported
        .into_iter()
        .map(|(line, kind, written)| (line, Finding::Unsupported { kind, written }))
        .collect();
    for (line, path) in found.paths {
        let written = path.written;
        let finding = match contract.verdict(&path.segments, gate) {
            Verdict::OnSurface => continue,
            Verdict::Outside { .. } if exempt_lines.contains(&line) => continue,
            Verdict::Outside { replacement } => Finding::Outside {
                path: written,
                replacement,
            },
            Verdict::GatedChild { child, gate } => Finding::GatedChild {
                path: written,
                child,
                gate,
            },
            Verdict::PackageModule => Finding::PackageModule { path: written },
        };
        findings.push((line, finding));
    }
    findings.sort_by_key(|(line, _)| *line);

    let file = Path::new("packages")
        .join(package)
        .join("src")
        .join(relative_path);
    findings
        .into_iter()
        .map(|(line, finding)| Diagnostic {
            package: package.to_string(),
            file: file.clone(),
            line,
            finding,
        })
        .collect()
}
