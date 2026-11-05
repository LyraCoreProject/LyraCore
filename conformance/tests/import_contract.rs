use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use syn::visit::{self, Visit};
use syn::{Attribute, Item, UseTree};

#[derive(Default)]
struct References {
    paths: Vec<Vec<String>>,
    methods: BTreeSet<String>,
    imports: BTreeMap<String, Vec<String>>,
    db_handles: BTreeSet<String>,
}

fn imports(tree: &UseTree, prefix: Vec<String>, names: &mut BTreeMap<String, Vec<String>>) {
    match tree {
        UseTree::Path(path) => {
            let mut prefix = prefix;
            prefix.push(path.ident.to_string());
            imports(&path.tree, prefix, names);
        }
        UseTree::Name(name) => {
            let mut target = prefix;
            if name.ident != "self" {
                target.push(name.ident.to_string());
            }
            names.insert(target.last().unwrap().clone(), target);
        }
        UseTree::Rename(rename) => {
            let mut target = prefix;
            target.push(rename.ident.to_string());
            names.insert(rename.rename.to_string(), target);
        }
        UseTree::Group(group) => {
            for tree in &group.items {
                imports(tree, prefix.clone(), names);
            }
        }
        UseTree::Glob(_) => {
            names.insert(format!("*{}", names.len()), prefix);
        }
    }
}

fn database_receiver(expression: &syn::Expr, handles: &BTreeSet<String>) -> bool {
    match expression {
        syn::Expr::Path(path) => path
            .path
            .get_ident()
            .is_some_and(|name| name == "ctx" || handles.contains(&name.to_string())),
        syn::Expr::Field(field) => database_receiver(&field.base, handles),
        syn::Expr::MethodCall(call) => database_receiver(&call.receiver, handles),
        syn::Expr::Reference(reference) => database_receiver(&reference.expr, handles),
        _ => false,
    }
}

impl<'ast> Visit<'ast> for References {
    fn visit_path(&mut self, path: &'ast syn::Path) {
        self.paths.push(
            path.segments
                .iter()
                .map(|part| part.ident.to_string())
                .collect(),
        );
        visit::visit_path(self, path);
    }

    fn visit_local(&mut self, local: &'ast syn::Local) {
        if let (syn::Pat::Ident(name), Some(initializer)) = (&local.pat, &local.init) {
            if database_receiver(&initializer.expr, &self.db_handles) {
                self.db_handles.insert(name.ident.to_string());
            }
        }
        visit::visit_local(self, local);
    }

    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        let name = call.method.to_string();
        if name.starts_with("game_")
            || (!database_receiver(&call.receiver, &self.db_handles)
                && !matches!(
                    name.as_str(),
                    "parse" | "collect" | "into" | "clone" | "to_owned"
                ))
        {
            self.methods.insert(name);
        }
        visit::visit_expr_method_call(self, call);
    }

    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        imports(&item.tree, Vec::new(), &mut self.imports);
    }

    fn visit_attribute(&mut self, _: &'ast Attribute) {}

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        self.visit_path(&mac.path);
        // Macro arguments can contain calls, paths and nested macros.
        let tokens = mac.tokens.to_string();
        let mut path = Vec::new();
        let mut colon = false;
        for word in tokens.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':')) {
            if word.is_empty() {
                continue;
            }
            if word == "::" {
                colon = true;
            } else {
                if !colon && !path.is_empty() {
                    self.paths.push(std::mem::take(&mut path));
                }
                path.extend(
                    word.split("::")
                        .filter(|part| !part.is_empty())
                        .map(str::to_owned),
                );
                colon = word.ends_with("::");
            }
        }
        if !path.is_empty() {
            self.paths.push(path);
        }
    }
}

struct Definition {
    file: PathBuf,
    module: Vec<String>,
    references: References,
}

#[derive(Default)]
struct Graph {
    definitions: BTreeMap<Vec<String>, Definition>,
    imports: BTreeMap<Vec<String>, BTreeMap<String, Vec<String>>>,
    modules: BTreeMap<Vec<String>, PathBuf>,
    methods: BTreeMap<String, Vec<Vec<String>>>,
    accessors: BTreeMap<String, Vec<String>>,
    roots: Vec<Vec<String>>,
    resolutions: RefCell<BTreeMap<(Vec<String>, Vec<String>), BTreeSet<Vec<String>>>>,
}

fn test_only(attributes: &[Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        attribute.path().is_ident("cfg")
            && matches!(&attribute.meta, syn::Meta::List(list) if list.tokens.to_string() == "test")
    })
}

impl Graph {
    fn read(&mut self, file: &Path, module: Vec<String>) {
        let parsed = syn::parse_file(&fs::read_to_string(file).unwrap()).unwrap();
        self.items(file, module, parsed.items);
    }

    fn items(&mut self, file: &Path, module: Vec<String>, items: Vec<Item>) {
        self.modules.insert(module.clone(), file.to_owned());
        for item in &items {
            if let Item::Use(item) = item {
                if !test_only(&item.attrs) {
                    imports(
                        &item.tree,
                        Vec::new(),
                        self.imports.entry(module.clone()).or_default(),
                    );
                }
            }
        }
        for item in items {
            let (name, attributes) = match &item {
                Item::Fn(item) => (Some(item.sig.ident.to_string()), &item.attrs),
                Item::Const(item) => (Some(item.ident.to_string()), &item.attrs),
                Item::Static(item) => (Some(item.ident.to_string()), &item.attrs),
                Item::Struct(item) => (Some(item.ident.to_string()), &item.attrs),
                Item::Enum(item) => (Some(item.ident.to_string()), &item.attrs),
                Item::Type(item) => (Some(item.ident.to_string()), &item.attrs),
                Item::Trait(item) => (Some(item.ident.to_string()), &item.attrs),
                Item::Macro(item) => (item.ident.as_ref().map(ToString::to_string), &item.attrs),
                Item::Mod(item) => {
                    if test_only(&item.attrs) {
                        continue;
                    }
                    let mut child = module.clone();
                    child.push(item.ident.to_string());
                    if let Some((_, items)) = &item.content {
                        self.items(file, child, items.clone());
                    } else {
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
                            Some(file.parent().unwrap().join(value.value()))
                        });
                        let directory = if matches!(
                            file.file_name().unwrap().to_str().unwrap(),
                            "mod.rs" | "lib.rs"
                        ) {
                            file.parent().unwrap().to_owned()
                        } else {
                            file.with_extension("")
                        };
                        let child_file = explicit.unwrap_or_else(|| {
                            let flat = directory.join(format!("{}.rs", item.ident));
                            if flat.is_file() {
                                flat
                            } else {
                                directory.join(item.ident.to_string()).join("mod.rs")
                            }
                        });
                        self.read(&child_file, child);
                    }
                    continue;
                }
                Item::Impl(item) => {
                    if test_only(&item.attrs) {
                        continue;
                    }
                    let syn::Type::Path(owner) = &*item.self_ty else {
                        continue;
                    };
                    let owner = owner.path.segments.last().unwrap().ident.to_string();
                    for method in &item.items {
                        if let syn::ImplItem::Fn(method) = method {
                            if test_only(&method.attrs) {
                                continue;
                            }
                            let mut name = module.clone();
                            name.extend([owner.clone(), method.sig.ident.to_string()]);
                            let mut references = References::default();
                            references.visit_impl_item_fn(method);
                            if method.sig.receiver().is_some() {
                                self.methods
                                    .entry(method.sig.ident.to_string())
                                    .or_default()
                                    .push(name.clone());
                            }
                            self.definitions.insert(
                                name,
                                Definition {
                                    file: file.to_owned(),
                                    module: module.clone(),
                                    references,
                                },
                            );
                        }
                    }
                    continue;
                }
                _ => continue,
            };
            if test_only(attributes) {
                continue;
            }
            let Some(name) = name else {
                continue;
            };
            let mut qualified = module.clone();
            qualified.push(name.clone());
            if let Item::Fn(item) = &item {
                if (name.starts_with("import_")
                    && module.first().is_none_or(|part| part != "transfer"))
                    || (module == ["seed"] && name == "init")
                    || name == "debug_repair_after_publish"
                {
                    if !item
                        .attrs
                        .iter()
                        .any(|attribute| attribute.path().is_ident("reducer"))
                    {
                        continue;
                    }
                    self.roots.push(qualified.clone());
                }
            }
            for attribute in attributes {
                if attribute.path().is_ident("table") {
                    if let syn::Meta::List(list) = &attribute.meta {
                        let tokens = list.tokens.to_string();
                        let parts: Vec<_> = tokens.split_whitespace().collect();
                        if let Some(index) = parts.iter().position(|part| *part == "accessor") {
                            self.accessors.insert(
                                parts[index + 2].trim_end_matches(',').to_owned(),
                                qualified.clone(),
                            );
                        }
                    }
                }
            }
            let mut references = References::default();
            references.visit_item(&item);
            self.definitions.insert(
                qualified,
                Definition {
                    file: file.to_owned(),
                    module: module.clone(),
                    references,
                },
            );
        }
    }

    fn resolve(
        &self,
        module: &[String],
        path: &[String],
        seen: &mut BTreeSet<Vec<String>>,
    ) -> BTreeSet<Vec<String>> {
        let key = (module.to_vec(), path.to_vec());
        if let Some(result) = self.resolutions.borrow().get(&key) {
            return result.clone();
        }
        let result = self.resolve_uncached(module, path, seen);
        self.resolutions.borrow_mut().insert(key, result.clone());
        result
    }

    fn resolve_uncached(
        &self,
        module: &[String],
        path: &[String],
        seen: &mut BTreeSet<Vec<String>>,
    ) -> BTreeSet<Vec<String>> {
        if path.is_empty() {
            return BTreeSet::new();
        }
        if matches!(
            path[0].as_str(),
            "std"
                | "core"
                | "spacetimedb"
                | "log"
                | "serde_json"
                | "lyracore_shared"
                | "lyracore_protocol"
                | "lyracore_package_delta"
                | "wow_world_messages"
                | "wow_world_base"
        ) {
            return BTreeSet::new();
        }
        assert!(seen.len() < 128, "resolution cycle: {module:?} {path:?}");
        let mut scope = module.to_vec();
        let mut remaining = path;
        if remaining[0] == "crate" {
            scope.clear();
            remaining = &remaining[1..];
        } else if remaining[0] == "self" {
            remaining = &remaining[1..];
        }
        while remaining.first().is_some_and(|part| part == "super") {
            scope.pop();
            remaining = &remaining[1..];
        }
        if remaining.is_empty() {
            return BTreeSet::new();
        }
        let mut qualified = scope.clone();
        qualified.extend_from_slice(remaining);
        let mut request = scope.clone();
        request.push("@".to_owned());
        request.extend_from_slice(remaining);
        if !seen.insert(request) {
            return BTreeSet::new();
        }
        if self.definitions.contains_key(&qualified) {
            return BTreeSet::from([qualified]);
        }
        if let Some(imports) = self.imports.get(&scope) {
            if let Some(target) = imports.get(&remaining[0]) {
                let mut target = target.clone();
                target.extend_from_slice(&remaining[1..]);
                return self.resolve(&scope, &target, seen);
            }
        }
        let mut child = scope.clone();
        child.push(remaining[0].clone());
        if self.modules.contains_key(&child) {
            return self.resolve(&child, &remaining[1..], seen);
        }
        if self.definitions.contains_key(&child) {
            return BTreeSet::from([child]);
        }
        let mut found = BTreeSet::new();
        if let Some(imports) = self.imports.get(&scope) {
            for (name, target) in imports {
                if name.starts_with('*') {
                    let mut target = target.clone();
                    target.extend_from_slice(remaining);
                    found.extend(self.resolve(&scope, &target, seen));
                }
            }
        }
        found
    }

    fn reachable(&self) -> BTreeMap<PathBuf, Vec<String>> {
        let mut files = BTreeMap::new();
        let mut visited = BTreeSet::new();
        let mut pending: Vec<_> = self
            .roots
            .iter()
            .map(|name| (name.clone(), vec![name.join("::")]))
            .collect();
        while let Some((name, chain)) = pending.pop() {
            if !visited.insert(name.clone()) {
                continue;
            }
            let definition = &self.definitions[&name];
            files
                .entry(definition.file.clone())
                .or_insert_with(|| chain.clone());
            for length in 0..=definition.module.len() {
                if let Some(file) = self.modules.get(&definition.module[..length]) {
                    files.entry(file.clone()).or_insert_with(|| chain.clone());
                }
            }
            let mut dependencies = BTreeSet::new();
            for path in &definition.references.paths {
                let mut path = path.clone();
                if let Some(target) = definition.references.imports.get(&path[0]) {
                    path = target
                        .iter()
                        .cloned()
                        .chain(path[1..].iter().cloned())
                        .collect();
                }
                dependencies.extend(self.resolve(&definition.module, &path, &mut BTreeSet::new()));
            }
            for method in &definition.references.methods {
                if let Some(target) = self.accessors.get(method) {
                    dependencies.insert(target.clone());
                }
                if let Some(targets) = self.methods.get(method) {
                    dependencies.extend(targets.iter().cloned());
                }
            }
            for dependency in dependencies {
                let mut chain = chain.clone();
                chain.push(dependency.join("::"));
                pending.push((dependency, chain));
            }
        }
        files
    }
}

fn contract() -> BTreeSet<PathBuf> {
    let output = Command::new("python3").args(["-B", "-c",
        "import sys,json; sys.path.insert(0,'conformance'); import audit; print(json.dumps([str(f) for p in audit.import_contract_paths() for f in (p.rglob('*') if p.is_dir() else [p]) if f.is_file()]))"])
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn import_and_initialization_dependencies_are_hashed() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut graph = Graph::default();
    graph.read(&root.join("module/src/lib.rs"), Vec::new());
    let hashed = contract();
    let reached = graph.reachable();
    if let Ok(path) = std::env::var("LYRACORE_IMPORT_GRAPH") {
        fs::write(
            path,
            serde_json::to_string_pretty(&reached.keys().collect::<Vec<_>>()).unwrap(),
        )
        .unwrap();
    }
    let missing: Vec<_> = reached
        .into_iter()
        .filter(|(file, _)| !hashed.contains(file))
        .map(|(file, chain)| {
            format!(
                "{}: {}",
                file.strip_prefix(root).unwrap().display(),
                chain.join(" -> ")
            )
        })
        .collect();
    assert!(
        missing.is_empty(),
        "unhashed import or initialization dependencies:\n{}",
        missing.join("\n")
    );
}
