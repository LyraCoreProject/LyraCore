# fire_nova

The worked example of a DATA-ONLY Package: no Rust, no client half, one Package Delta and one
Script Artifact.

Its data is authored in `datascripts/src/fire_nova/spells.ts`, which clones a real spell into a
five-rank ladder of Package spells. Its one Runtime Script is `scripts/ember_echo.ts`, which ships
switched off. Read those two files — they are the example.

Nothing under `data/` is committed. `lyracore packages build` runs the Datascript, compiles the
Runtime Scripts and writes both artifacts; the importer reapplies them after every spell import.
Build them yourself:

```
lyracore-importer --dbc <client Data/ dir> --spell-snapshot datascripts/generated/base-snapshot.json
bun run datascripts/src/fire_nova/spells.ts
bun run datascripts/runtime-scripts/build-scripts.ts fire_nova
lyracore-delta-check packages/fire_nova/data/.generated/*.json
```

An unmodified client shows no tooltip for a Package spell: it renders spells from its own
`Spell.dbc`, which has never heard of one. The identifiers are safe because nothing else can own
them, not because the client knows them.
