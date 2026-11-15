use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use syn::parse::Parser;
use syn::visit::{self, Visit};
use syn::{Attribute, ItemMod};

#[path = "../support/scratch.rs"]
mod scratch;

fn test_only(attributes: &[Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        attribute.path().is_ident("cfg")
            && matches!(&attribute.meta, syn::Meta::List(list) if list.tokens.to_string() == "test")
    })
}

struct Sources {
    file: PathBuf,
    directory: PathBuf,
    inline: bool,
    files: BTreeSet<PathBuf>,
    inputs: BTreeSet<PathBuf>,
    tables: BTreeMap<String, PathBuf>,
    table_references: BTreeSet<String>,
}

impl Sources {
    fn read(file: &Path) -> Self {
        let file = file.canonicalize().unwrap();
        let directory = if matches!(
            file.file_name().unwrap().to_str().unwrap(),
            "lib.rs" | "main.rs" | "mod.rs" | "build.rs"
        ) {
            file.parent().unwrap().to_owned()
        } else {
            file.with_extension("")
        };
        let source = syn::parse_file(&fs::read_to_string(&file).unwrap()).unwrap();
        let mut scan = Self {
            files: BTreeSet::from([file.clone()]),
            file,
            directory,
            inline: false,
            inputs: BTreeSet::new(),
            tables: BTreeMap::new(),
            table_references: BTreeSet::new(),
        };
        scan.visit_file(&source);
        scan
    }

    fn include_path(&self, mac: &syn::Macro) -> Option<PathBuf> {
        if let Ok(path) = syn::parse2::<syn::LitStr>(mac.tokens.clone()) {
            return Some(self.file.parent().unwrap().join(path.value()));
        }
        let expression: syn::ExprMacro =
            syn::parse2(mac.tokens.clone()).expect("review computed include input");
        assert!(
            expression.mac.path.is_ident("concat"),
            "review computed include input"
        );
        let args = syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated
            .parse2(expression.mac.tokens)
            .unwrap();
        assert_eq!(args.len(), 2, "review computed include input");
        let syn::Expr::Macro(environment) = &args[0] else {
            panic!("review computed include input")
        };
        assert!(environment.mac.path.is_ident("env"));
        let name: syn::LitStr = syn::parse2(environment.mac.tokens.clone()).unwrap();
        let syn::Expr::Lit(suffix) = &args[1] else {
            panic!("review computed include input")
        };
        let syn::Lit::Str(suffix) = &suffix.lit else {
            panic!("review computed include input")
        };
        match name.value().as_str() {
            "OUT_DIR" if mac.path.is_ident("include") => None,
            "CARGO_MANIFEST_DIR" => {
                let directory = self
                    .file
                    .ancestors()
                    .find(|path| path.join("Cargo.toml").is_file())
                    .unwrap();
                Some(directory.join(suffix.value().trim_start_matches('/')))
            }
            _ => panic!("review computed include input in {}", self.file.display()),
        }
    }
}

impl<'ast> Visit<'ast> for Sources {
    fn visit_item_mod(&mut self, item: &'ast ItemMod) {
        if test_only(&item.attrs) {
            return;
        }
        if item.content.is_some() {
            let directory = self.directory.clone();
            let inline = self.inline;
            self.inline = true;
            self.directory.push(item.ident.to_string());
            visit::visit_item_mod(self, item);
            self.directory = directory;
            self.inline = inline;
            return;
        }
        let explicit = item.attrs.iter().find_map(|attribute| {
            if !attribute.path().is_ident("path") {
                return None;
            }
            let syn::Meta::NameValue(value) = &attribute.meta else {
                return None;
            };
            let syn::Expr::Lit(value) = &value.value else {
                return None;
            };
            let syn::Lit::Str(value) = &value.lit else {
                return None;
            };
            let directory = if self.inline {
                self.directory.as_path()
            } else {
                self.file.parent().unwrap()
            };
            Some(directory.join(value.value()))
        });
        let path = explicit.unwrap_or_else(|| {
            let flat = self.directory.join(format!("{}.rs", item.ident));
            if flat.is_file() {
                flat
            } else {
                self.directory.join(item.ident.to_string()).join("mod.rs")
            }
        });
        let child = Self::read(&path);
        self.files.extend(child.files);
        self.inputs.extend(child.inputs);
        self.tables.extend(child.tables);
        self.table_references.extend(child.table_references);
    }

    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        if !test_only(&item.attrs) {
            visit::visit_item_fn(self, item);
        }
    }

    fn visit_item_const(&mut self, item: &'ast syn::ItemConst) {
        if !test_only(&item.attrs) {
            visit::visit_item_const(self, item);
        }
    }

    fn visit_item_struct(&mut self, item: &'ast syn::ItemStruct) {
        if test_only(&item.attrs) {
            return;
        }
        for attribute in &item.attrs {
            if attribute
                .path()
                .segments
                .last()
                .is_some_and(|part| part.ident == "table")
            {
                if let syn::Meta::List(list) = &attribute.meta {
                    let tokens = list.tokens.to_string();
                    let parts: Vec<_> = tokens.split_whitespace().collect();
                    if let Some(index) = parts.iter().position(|part| *part == "accessor") {
                        self.tables.insert(
                            parts[index + 2].trim_end_matches(',').to_owned(),
                            self.file.clone(),
                        );
                    }
                }
            }
        }
        visit::visit_item_struct(self, item);
    }

    fn visit_lit_str(&mut self, literal: &'ast syn::LitStr) {
        self.table_references.extend(table_names(&literal.value()));
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        let tokens = mac.tokens.to_string();
        self.table_references.extend(table_names(&tokens));
        if mac.path.is_ident("include_str")
            || mac.path.is_ident("include_bytes")
            || mac.path.is_ident("include")
        {
            if let Some(path) = self.include_path(mac) {
                let path = path.canonicalize().unwrap();
                self.inputs.insert(path.clone());
                if mac.path.is_ident("include") {
                    let child = Self::read(&path);
                    self.files.extend(child.files);
                    self.inputs.extend(child.inputs);
                    self.tables.extend(child.tables);
                    self.table_references.extend(child.table_references);
                }
            }
        } else {
            assert!(
                !["include !", "include_str !", "include_bytes !"]
                    .iter()
                    .any(|marker| tokens.contains(marker)),
                "review nested input macro in {}",
                self.file.display()
            );
        }
    }
}

fn table_names(source: &str) -> BTreeSet<String> {
    source
        .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .filter(|name| name.starts_with("game_"))
        .map(str::to_owned)
        .collect()
}

fn contracts() -> (BTreeSet<PathBuf>, BTreeSet<PathBuf>) {
    let output = Command::new("python3").args(["-B", "-c", r#"
import sys,json
sys.path.insert(0,'conformance')
import audit
strict = [str(f) for p in audit.import_contract_paths() for f in (p.rglob('*') if p.is_dir() else [p]) if f.is_file()]
print(json.dumps([strict, [str(audit.ROOT / name) for name in audit.gameplay_manifest()]]))
"#]).current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn missing(
    scan: &Sources,
    strict: &BTreeSet<PathBuf>,
    gameplay: &BTreeSet<PathBuf>,
) -> BTreeSet<PathBuf> {
    scan.files
        .difference(&strict.union(gameplay).cloned().collect())
        .cloned()
        .chain(scan.inputs.difference(strict).cloned())
        .collect()
}

#[test]
fn import_and_initialization_dependencies_are_in_a_world_tier() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut scan = Sources::read(&root.join("module/src/lib.rs"));
    let build = Sources::read(&root.join("module/build.rs"));
    scan.files.extend(build.files);
    scan.inputs.extend(build.inputs);
    let importer = Sources::read(&root.join("importer/src/main.rs"));
    let mut imported_tables = importer.table_references.clone();
    scan.files.extend(importer.files);
    scan.inputs.extend(importer.inputs);
    for entry in [
        "importer/src/lib.rs",
        "crates/lyracore-shared/src/lib.rs",
        "crates/lyracore-package-delta/src/lib.rs",
        "crates/lyracore-protocol/src/lib.rs",
    ] {
        let source = Sources::read(&root.join(entry));
        if entry == "crates/lyracore-package-delta/src/lib.rs" {
            imported_tables.extend(source.table_references);
        }
        scan.files.extend(source.files);
        scan.inputs.extend(source.inputs);
    }
    let (strict, gameplay) = contracts();
    assert!(imported_tables.contains("game_creature_spawn"));
    let nonstrict_tables: BTreeMap<_, _> = imported_tables
        .iter()
        .filter_map(|name| {
            scan.tables
                .get(name)
                .filter(|file| !strict.contains(*file))
                .map(|file| (name, file))
        })
        .collect();
    assert!(
        nonstrict_tables.is_empty(),
        "imported table definitions must be in Tier A: {nonstrict_tables:#?}"
    );
    let uncovered = missing(&scan, &strict, &gameplay);
    assert!(
        uncovered.is_empty(),
        "unhashed import or initialization sources or data: {uncovered:#?}"
    );
    for required in [
        "module/src/seed/fixtures.rs",
        "module/src/creatures/ai.rs",
        "module/src/creatures/spawn.rs",
    ] {
        let path = root.join(required);
        let mut strict = strict.clone();
        let mut gameplay = gameplay.clone();
        strict.remove(&path);
        gameplay.remove(&path);
        assert!(
            missing(&scan, &strict, &gameplay).contains(&path),
            "coverage must detect removal of {required}"
        );
    }
}

#[test]
fn external_modules_and_included_data_cannot_escape_coverage() {
    let directory = scratch::Scratch::new("import-contract").unwrap();
    let root = directory.to_path_buf();
    fs::create_dir(root.join("inline")).unwrap();
    fs::write(
        root.join("lib.rs"),
        r#"
mod inline { #[path = "branch.rs"] mod branch; }
#[cfg(test)] mod absent;
// mod commented_out;
"#,
    )
    .unwrap();
    fs::write(root.join("inline/branch.rs"), "include!(\"nested.rs\");").unwrap();
    fs::write(
        root.join("inline/nested.rs"),
        "const BYTES: &[u8] = include_bytes!(\"data.bin\");",
    )
    .unwrap();
    fs::write(root.join("inline/data.bin"), b"world input").unwrap();
    let scan = Sources::read(&root.join("lib.rs"));
    let strict = BTreeSet::from([
        root.join("lib.rs"),
        root.join("inline/branch.rs"),
        root.join("inline/nested.rs"),
    ]);
    assert_eq!(
        missing(&scan, &strict, &BTreeSet::new()),
        BTreeSet::from([root.join("inline/data.bin")])
    );
}
