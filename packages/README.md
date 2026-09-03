# packages/

Every folder here is an ENABLED Package: `module/build.rs` compiles its `src/` into the module wasm
and `lyracore client sync` packs its `client/` into your game client. No manifest lists them — this
directory listing is the enabled set.

`example/` is the maintained reference Package. `lyracore packages new <name>` copies and renames it
to scaffold a new one; read its `src/mod.rs` for the structure a Package's Rust half follows.

`docs/package-api.md` is the Package API: the versioned list of what a Package's Rust half may call,
and what core promises about it. The build lints every file here against that list and fails on a
path outside it, naming the Package, the file and the line.

A Package may also have a DATA half: `data/.generated/*.json`, the Package Deltas a Datascript
generates, which the importer reapplies after every base import. `fire_nova/` is the worked example;
its Datascript lives at `datascripts/src/fire_nova/spells.ts`, because only artifacts belong inside
a Package folder. Any one half — `src/`, `client/` or `data/` — is a valid Package on its own.

A Package that must change a stock FrameXML or GlueXML file declares the edit in
`client/ui-transforms.json` instead of shipping a whole replacement under `client/mpq/`. Each entry
names a path under `Interface/FrameXML/` or `Interface/GlueXML/`, one anchor (`before`, `after` or
`replace`) that must occur exactly once in the file, and the text to insert. Two Packages may edit
one file while their anchors stay apart. `lyracore client sync` composes the edits against your own
client's stock bytes, which makes the result baseline-derived: it reaches your client only, and
`lyracore client pack` refuses to put it in a distributable artifact. See
`docs/development-cli.md` for the anchors, the conflict rules and the load order.

`lyracore packages disable <name>` moves a folder out of here into `.lyracore/packages-disabled/`,
where the build cannot see it, and `lyracore packages enable <name>` moves it back. The location is
the enabled state, so this listing stays the whole truth about what compiles.

`lyracore packages add <git-url>` installs a Package from a repository whose root is the Package
itself, and records the commit it came from. `lyracore packages update <name>` advances that Package
to the repository's current commit, keeping the old folder until the new one preflights.

`lyracore packages add <name>` installs a first-party Package by bare name instead, resolved from
the Official Package Collection (`LyraCoreProject/packages`) and pinned to the commit it was
resolved at. `packages update` does not advance this kind.

See `docs/development-cli.md` for `lyracore packages add`, `list`, `new`, `enable`, `disable`,
`remove`, and `update`.

## Operator-tunable config

A Package that wants a value the Operator can change without a republish seeds it as Package
Config: call `crate::package_config::ensure_package_config_default(ctx, "<your package>", "<key>",
"<default value>")` from your own ensure/init path, every time it runs. The call only inserts when
the row is absent, so a repeated call never clobbers a value the Operator has since edited.
`spacetime sql "select * from game_package_config"` then shows real keys with live values, not a
blank slate someone has to populate by hand.

The Operator changes a value with the `set_package_config` reducer (`package_name, key, value,
allow_new`). It refuses an unknown `(package_name, key)` pair — naming the package's existing keys —
unless `allow_new` is set, so a typo in the key name fails loud instead of writing a key nobody
reads. A dedicated CLI verb for this reducer is planned (#370); until then, call it directly with
`spacetime call`.
