# packages/

Every folder here is an ENABLED Package: `module/build.rs` compiles its `src/` into the module wasm
and `lyracore client sync` packs its `client/` into your game client. No manifest lists them — this
directory listing is the enabled set.

`example/` is the maintained reference Package. `lyracore packages new <name>` copies and renames it
to scaffold a new one; read its `src/mod.rs` for the structure a Package's Rust half follows.

`lyracore packages disable <name>` moves a folder out of here into `.lyracore/packages-disabled/`,
where the build cannot see it, and `lyracore packages enable <name>` moves it back. The location is
the enabled state, so this listing stays the whole truth about what compiles.

See `docs/development-cli.md` for `lyracore packages add`, `list`, `new`, `enable`, `disable`, and
`remove`.
