# Native host experiment results

These results describe the initial prototype. The [full Module implementation](../../docs/architecture.md#native-host-migration)
and its verification were added afterward.

The native host route works for this movement and AOI fixture. I would continue with entity grants maintained by the Module and one shared native delivery feed. That design removed the Gateway process and sustained the tested 1,000-client workload. A full Gateway replacement remains unproven.

The successful path is:

```text
WoW movement packet
  -> native SpacetimeDB endpoint
  -> Module movement and AOI grants
  -> shared internal movement + grant subscriptions
  -> recipient queues, world packets, encrypted headers
```

The Module decides recipients. The host performs protocol handling and delivery, with a small movement cache and reverse recipient index. No external WebSocket, SDK cache, or Gateway process sits on this path.

## Final measurements

These are final implementation revision 3 runs on one four-core i7-7820HQ. Clients send native encrypted movement at 10 Hz for ten seconds. Spread scenes contain groups of up to 32 visible Characters. Host and driver share the machine. Delivery counts include a one-second drain period.

| Native clients | Scene | Runs | Recipient updates per run | p99 latency | All updates received |
| --- | --- | --- | --- | --- | --- |
| 128 | Everyone sees everyone | 1 | 1,638,400 | 42 ms | Yes |
| 500 | Groups of up to 32 | 3 | 1,576,000 | 43, 48, 43 ms | Yes, all three |
| 1,000 | Groups of up to 32 | 3 | 3,180,800 | 73, 74, 78 ms | Yes, all three |

The 1,000-client workload submits 10,000 movement requests per second. The host averaged 2.70 to 2.73 CPU cores over measurement and drain, with sampled RSS of 270 to 286 MiB. These are local fixture results, not production capacity estimates.

The final 32-client correctness run also passed all twelve checks, including world proof refusal, Character ownership, duplicate World Session refusal, space isolation, hidden and ghost-only entities, viewer ghost changes, native cell entry and exit, ordering, and reconnect snapshots. Three independent vanilla decoder tests passed.

## What failed

Per-viewer query-backed views are expensive in SpacetimeDB 2.13.0. An early 500-client spread run spent about 36 seconds computing views during 45 seconds of processing. It made 200,000 view calls. A later run with the final indexes also missed the target. The source confirms whole-view recomputation on source table changes; the [methodology](README.md#designs-tested) links the exact upstream code.

Direct incremental SQL per viewer removed that recomputation but retained expensive joins. In the indexed batch comparison, both cell grants and entity grants kept up at 128 spread clients and fell behind at 500. Native ingress with per-viewer cell joins also fell behind at 128 and 500. The shared feed avoids those per-viewer subscription queries entirely.

The final shared-feed design still has measured limits:

- **Dense delivery.** With 500 Characters all seeing one another, batched ingress offered 25 million recipient updates. Only 19,669,652 had arrived by the drain deadline, and p99 among received updates was 13.6 seconds. Both host delivery and the two-thread driver need profiling before attributing this limit to either one alone.
- **Grant churn.** Moving 128 dense Characters across cells at 10 Hz took 12.1 seconds to process ten seconds of scheduled work. All 1,638,400 recipient updates arrived, but p99 was 2.21 seconds. Files under the disposable data directory grew by about 191.5 MB. This Module deletes and rebuilds grants; the result is a limit of that algorithm, not proof that all AOI maintained by the Module must cost this much.
- **Shared-feed overload.** At 2,000 native clients, the host filled its 16,384-message internal client queue and stopped the shared feed. The world connections then closed. That checks the final failure behavior, but it does not establish a useful capacity figure at 2,000 clients.

An intermediate version yielded after every feed message. It overflowed the internal queue at 1,000 clients and left world connections without updates. The final version yields after batches of up to 32 messages, uses bounded recipient queues, and closes affected World Sessions when the feed task exits or is aborted. The three successful 1,000-client repeats use that final version. The evidence archive retains the unsuccessful versions too.

## What I would build next

Keep AOI authority in the Module and shared delivery inside the native host. The next useful work is incremental grant maintenance with stable membership keys, batching socket writes without dropping movement updates, and a real Character login/movement/combat slice using the actual game Module. Then run the existing Gateway load suite against both paths with the same scene and hardware.

The world endpoint currently uses `wow_srp` with a public fixture session key. It proves world proof checking, header encryption, movement, creation, and destruction. It does not implement full logon SRP, Character selection, complete object fields, combat, cross-shard Transfer, or a real graphical client login. The existing LyraCore code still pins SpacetimeDB 2.7.1; these experiments use a modified 2.13.0 host.

No controlled A/B against today's Gateway has run. This experiment establishes a feasible architecture and exposes its first limits. It does not yet prove a speedup over the existing Realm.

## Evidence and reproduction

[README.md](README.md) documents the setup, variants, measurement rules, scope, and commands. [results/summary.json](results/summary.json) contains each recorded run and selected metric deltas. [results/raw.tar.gz](results/raw.tar.gz) contains raw result JSON, histograms, batch timings, metrics, and relevant warnings, without signing keys or database files. Filter `implementation_revision = 3` for the final shared-feed results. Earlier runs record the alternatives and failed intermediate versions.

Final review passed for the new Rust, Python, TOML, and host integration templates. Formatting, workspace Clippy with warnings denied, three codec tests, Python compilation, the native release build, and the final correctness run passed. Comment review passed separately. The upstream host suite and LyraCore's full game suite were not run.
