# S3-pre reader hardening

Base: `cf48bb093e57533abe8a70bd2bc8971a08252ac6`. Branch:
`vmangos/s3-pre-reader-hardening`. PR target: `feat/vmangos-world-data`.

## Verified changes

- `parse_table` and `field` return errors through family builders. SQLite failures, missing
  columns and invalid patch ranges no longer reach an assumed classic-db contract and panic.
- Classic fixtures use the production numeric checks. Tests for nonnumeric cells now expect
  reader errors; numeric values outside a destination's range still test the relevant drop path.
  The nine quest constants used only in tests moved into the test module.
- `parse_complete_table` is gone. `WorldSource::ClassicDb` holds `ParsedDump`, so the census and
  importer use one contract-checked reader.
- The classic tokenizer preserves UTF-8 bytes through MySQL escape decoding. A regression test
  covers accented text, CJK, emoji, escaped newlines and apostrophes.
- Fetch permits an explicit `--accept-lock` to replace a lock whose GitHub comparison returns
  404. It still checks the asset digest and migration fingerprint before publication. The test
  proves failure without acceptance leaves the lock and report unchanged.

The ignored Alliance test fails on the untouched base with 307 emote dependencies against an
expectation of 300. Replaying the capped population selection from before `89c2663f` gives 300;
current population alternatives give 307. The extra rules are 54802, 57801, 117405, 117507,
117702, 142502 and 172603. The same change includes rule 600801, increasing action-56 rules from
14 to 15. Both expectations are repaired; the complete ignored test passes.

## UTF-8 impact and rebuilt baseline

The pinned input is 73,294,017 bytes, SHA-256
`d2083bcd2670451279cbf93af138eadae04c6d183a4cd0ff0357047e4a565de6`.
Of the 62 loaded tables, only `fishing_loot_template.comments` contains non-ASCII bytes.
Nineteen decoded cells change, reducing their combined storage by 66 bytes. Their `(entry,item)`
keys are:

`(16,11008) (28,11008) (41,11017) (47,11008) (139,11010) (361,11008) (618,11010)
(1222,11105) (1227,11105) (1977,11010) (3140,11105) (46,11010) (490,11008) (493,11008)
(1112,11010) (1377,11010) (2017,11010) (2057,11010) (2100,11008)`.

Those comments are not emitted. No SQL or reducer row changes. Two UTF-8 baseline runs match
each other and Milestone 1's fixed baseline byte for byte. The final reader hardening matches
that rebuilt baseline too:

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| full-world dry-run SQL | 71,151,072 | `b9cef3c0ca9acae9fa1994370d5f21fd4be0eb8faac90b27dc3457004a6baa40` |
| all 467 reducer batches, JSON | 9,718,119 | `e950ec5870e6d6be57e2c6b83765770318360a68354538ccef18f58d81c89098` |

Reducer capture used a temporary export at `DumpPlan::reducer_batches`; that instrumentation
was removed before the final release build. Final full-world and alliance-single SQL each pass
`cmp` against their UTF-8 baseline.

## Validation

- `cargo test -p lyracore-importer`: 483 passed, 16 ignored.
- The pinned `pinned_alliance_single_plan_is_apply_ready_with_relay_static_dependencies` test:
  1 passed, with `LYRACORE_CLASSIC_DB_SQL` and `LYRACORE_TEST_DBC` set.
- `cargo clippy -p lyracore-importer --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- `python3 -W error::ResourceWarning -m unittest discover -s importer/scripts/tests`:
  16 passed, without warnings.
- Comment review: passed for changed source, tests and fetch script, including retained comments
  around changed code. The summon lifetime units, proc cooldown units, SQL NULL ambiguity and
  reducer dependency order remain documented. Documentation pointers resolve.

Release measurements use Python `resource.getrusage(RUSAGE_CHILDREN).ru_maxrss` and
`time.monotonic`; `/usr/bin/time` is unavailable. Each invocation has a fresh wrapper process.

| Classic profile | Peak RSS | Wall time | Limit |
| --- | ---: | ---: | --- |
| full-world | 535.2 MiB | 8.50 s | 587.5 MiB / 14.375 s |
| alliance-single | 350.5 MiB | 7.25 s | 475 MiB / 12.125 s |

Local evidence is in `.import/vmangos-m2/`: baseline and final SQL, reducer captures, UTF-8 row
impact JSON, test logs, population-selection diagnostics and performance JSON. No source data
is included in this report.

## Limits and next slice

No stop rule triggered. No census counts or unsupported findings change in this prerequisite.
The 16 condition-gated portals remain in `unsupported.md`. No Module or schema code changed;
wasm, durable tests, a local Realm and a client were not run. vmangos import performance is not
available yet because production family import starts in S3a. The remaining Milestone 2 slices
must compare classic output to the hashes above and measure each supported vmangos family.
