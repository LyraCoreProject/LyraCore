# packages/

Every folder here is an ENABLED Package: `module/build.rs` compiles its `src/` into the module wasm
and `lyracore client sync` packs its `client/` into your game client. No manifest lists them — this
directory listing is the enabled set.

`example/` is the maintained reference Package. `lyracore packages new <name>` copies and renames it
to scaffold a new one; read its `src/mod.rs` for the structure a Package's Rust half follows.

See `docs/development-cli.md` for `lyracore packages add`, `list`, and `new`.
