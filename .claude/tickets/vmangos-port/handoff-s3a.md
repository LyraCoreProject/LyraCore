# S3a globals

Branch: `vmangos/s3a-globals`. Target: `vmangos/s3-pre-reader-hardening`, whose head is
`dbcb15f8`. This slice follows the reader prerequisite in the Milestone 2 stack.

## Verified implementation

`--source vmangos --family globals` now builds and emits a production globals plan from SQLite.
It uses the shared patch/build selection and typed named cells. The classic path uses the same
globals builders and retains its original output. The census checks its counts against those
builders. The source switch still defaults to classic-db; unported vmangos families are refused.

The Module checks graveyard area links before zone links. Faction, destination map and dungeon
destination checks run before fallback. The source reference is `GetClosestGraveYard` in
`src/game/ObjectMgr.cpp:7512-7524` at vmangos
`464179081673cfd240f7ffe7f0daf96bca0b5a70`, read through the GitHub contents API. No source code
was copied. No schema changed.

Hand-written SQLite fixtures cover every globals table, reordered columns, inclusive build and
patch ranges, portal version selection, and numeric type errors. A disposable local Realm test
proves that a farther area graveyard wins over the zone graveyard, and that a wrong faction,
wrong map or missing area link falls back to the zone.

## Generated census

`census/globals.md` was regenerated with the release importer, pinned classic SQL, pinned vmangos
SQLite and build-5875 client data. It contains one explanation per graveyard and portal difference.

| Module table | Classic | vmangos | Explanation |
| --- | ---: | ---: | --- |
| game_class_level_stats | 540 | 540 | Same keys. |
| game_level_stats | 2,400 | 2,400 | Same keys. |
| game_start_position | 40 | 40 | Same keys. |
| game_graveyard_zone | 191 | 189 | Ten source rows fall outside patch 10; selected links add 40 keys and remove 42. Twenty-four retained keys change faction. |
| game_createinfo_spell | 1,497 | 1,497 | Two source rows fall outside build 5875; selected keys match. |
| game_createinfo_action | 215 | 214 | One button key added, two removed. |
| game_start_item extras | 0 | 0 | All 302 vmangos rows overlap CharStartOutfit. |
| game_areatrigger_teleport | 103 | 115 | Version selection removes six rows; 13 trigger ids are added and one removed. One hundred common rows change only their names; their destinations match. |

All consumed globals floors pass. No new unsupported category was added. The production globals
Compatibility Manifest and census carry the existing 16 `globals.portal-condition` findings with
their trigger ids; `unsupported.md` now records that these rows are emitted. Their condition
behavior remains unsupported, and this is not a passed S6c Source Profile gate.

## Parity and performance

Full-world classic SQL and all 467 reducer batches match the rebuilt S3-pre baseline byte for
byte. Alliance classic SQL also matches. Hashes remain:

- SQL: `b9cef3c0ca9acae9fa1994370d5f21fd4be0eb8faac90b27dc3457004a6baa40`.
- Reducer JSON: `e950ec5870e6d6be57e2c6b83765770318360a68354538ccef18f58d81c89098`.

The reducer comparison used a temporary ignored test, removed after it passed. No capture hook
remains in production. The two vmangos profile runs emit identical globals SQL.

Release measurements use the same Python getrusage/monotonic wrapper as S3-pre:

| Source and scope | Profile | Peak RSS | Wall time |
| --- | --- | ---: | ---: |
| Classic, complete import | full-world | 534.34 MiB | 8.304 s |
| Classic, complete import | alliance-single | 348.89 MiB | 7.306 s |
| vmangos, globals only | full-world | 18.98 MiB | 0.0686 s |
| vmangos, globals only | alliance-single | 19.12 MiB | 0.0688 s |

Both classic runs meet the brief's limits. vmangos figures cover only the ported family; they
cannot establish the performance of a complete world import.

## Checks and limits

- Importer suite: 485 passed, 16 ignored.
- Module unit suite with `debug_reducers`: 1,546 passed.
- Module `build_script` and `package_api_lint` targets: passed.
- New `graveyard` durable target: 1 passed on its own local standalone. Its first run reached the
  correct graveyard but used a self-resurrection reset without that ability; the reset was fixed
  to use spirit-healer resurrection before the passing run.
- Workspace clippy, all targets and features, warnings denied: passed.
- Workspace fmt, diff whitespace check and published-feature wasm check: passed.
- The fetch script is unchanged from S3-pre's 16 passing script tests.
- Comment review passed for changed source and tests and retained comments around changed code.
  The dungeon coordinate constraint, reducer dependency order and starter-item id band remain
  accurate. The source reference and documentation links resolve.

No stop rule triggered. No production or deployed Realm was touched. The vmangos data import was
measured as a dry run; it was not applied to a Realm. The durable test uses hand-written rows.
No real-client check was performed. Local evidence is in `.import/vmangos-m2/s3a-*`.

S3b can extend the vmangos plan with the items family. Keep each source's consumed columns explicit,
keep the classic hashes above, and retain the globals findings through S3g. Milestone 3 must still
port creatures and spells; this slice does not change either catalogue.
