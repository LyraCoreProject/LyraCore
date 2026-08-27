# fire_nova

The worked example of a DATA-ONLY Package: no Rust, no client half, one Package Delta.

Its content is authored in `datascripts/src/fire_nova/spells.ts`, which clones a real spell into a
five-rank ladder of Package spells. Read that file — it is the example.

Nothing under `data/` is committed. `lyracore packages build` runs the Datascript and writes
`data/.generated/spell.json`, which the importer then reapplies after every spell import. Build it
yourself:

```
lyracore-importer --dbc <client Data/ dir> --spell-snapshot datascripts/generated/base-snapshot.json
bun run datascripts/src/fire_nova/spells.ts
lyracore-delta-check packages/fire_nova/data/.generated/spell.json
```

An unmodified client shows no tooltip for a Package spell: it renders spells from its own
`Spell.dbc`, which has never heard of one. The identifiers are safe because nothing else can own
them, not because the client knows them.
