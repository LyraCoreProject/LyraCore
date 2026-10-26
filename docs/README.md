# Docs

Each page has one job. When two pages seem to disagree, the code wins, then the page named as the
authority below. [Danger zones](./danger-zones.md) is the authority for traps, schema migrations
and publishing. [The Package API](./package-api.md) is the authority for the Package contract.

| Doc | Kind | Read it when |
| --- | --- | --- |
| [quickstart.md](./quickstart.md) | How-to | You set up a local Realm and connect a client |
| [windows.md](./windows.md) | How-to | You run the Realm in WSL2 on Windows |
| [triage.md](./triage.md) | Runbook | An agent helps a user diagnose a local problem |
| [danger-zones.md](./danger-zones.md) | Reference, authority | You change a schema, publish, or touch a §1 trap |
| [architecture.md](./architecture.md) | Reference | You need to know where a responsibility lives |
| [schema.md](./schema.md) | Reference | You add or change a table |
| [testing.md](./testing.md) | Reference | You run CI checks locally or choose a verification rung |
| [benilla-compatibility.md](./benilla-compatibility.md) | Reference | You change packet layouts or the opcode inventory |
| [native-operations.md](./native-operations.md) | Runbook | An Operator updates a deployed Realm |
| [operations/disk-policy.md](./operations/disk-policy.md) | Runbook | An Operator installs pruning and disk monitoring |
| [operations/vmap-rollout.md](./operations/vmap-rollout.md) | Runbook | A maintainer enables exact collision on a Realm |
| [data-ingestion.md](./data-ingestion.md) | How-to | You run importer stages or read a dry run |
| [operator-content-api.md](./operator-content-api.md) | Reference | A world editor edits spawns on a running Realm |
| [migrating-to-native-host.md](./migrating-to-native-host.md) | How-to | You port a Gateway-era branch, Package or local Realm |
| [package-api.md](./package-api.md) | Reference, authority | You write or review a Package |
| [package-delta-format.md](./package-delta-format.md) | Reference | You read or produce a Package Delta |
| [runtime-script-host.md](./runtime-script-host.md) | Reference | You change the Lua host for Runtime Scripts |
| [module-build.md](./module-build.md) | Reference | You change Package discovery in `module/build.rs` |
| [instance-lifecycle.md](./instance-lifecycle.md) | Reference | You change Instance leases, population or removal |
| [character-transfer.md](./character-transfer.md) | Reference | You touch the retained Transfer reducers |
| [recipes/add-an-opcode.md](./recipes/add-an-opcode.md) | How-to | You handle a new client opcode |
| [recipes/add-a-spell-effect.md](./recipes/add-a-spell-effect.md) | How-to | You implement a spell effect |
| [agents/maintainer.md](./agents/maintainer.md) | Runbook | An agent works for a maintainer |
| [agents/cross-repo-cli.md](./agents/cross-repo-cli.md) | How-to | A change spans this repository and `lyracore-cli` |
| [adr/](./adr/) | Decision records | You want the reason behind a standing decision |
| [verify-lyracore checks](../.claude/skills/verify-lyracore/checks.md) | Checklist | A change needs a check on a real 1.12.1 client |
