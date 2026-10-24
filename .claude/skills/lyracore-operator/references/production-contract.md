# Native Realm target contract

Use [the native operations guide](../../../../docs/native-operations.md) for the authoritative
procedure. This checkout has one Native Host and one Shard. Legacy Gateway deployment commands
are not applicable.

Require an approved configuration source independent of the running process, such as a reviewed
service unit and environment file. It must name the host, checkout, Native Host executable, service
manager, data directory, keys, log source, explicit HTTP URI, Shard and native listener addresses.
Record the intended commit and any other Realms on the machine. Do not infer these from contributor
defaults or a running process alone.

Compare three views before a write:

1. The approved configuration.
2. The process executable, start time, listener ownership and sanitized configuration.
3. The Shard inventory and advertised Realm address on that exact HTTP node.

Use the native publisher configuration for every `spacetime` command. Never print its token or
private authentication rows. The HTTP URI for publication must be explicit loopback with a nonzero
port. The CLI checks the recorded live Native Host for port 3000. Managed deployments on another
port must prove their process and node match the approved source.

Stop before mutation if the target is missing or ambiguous, the publisher identity is unavailable,
or these views disagree. Existing authorization carries forward when it already covers this exact
target and operation.

Prove health with process and socket checks, a fresh native world challenge, authenticated world
entry and a subsequent request. A listening socket or healthy HTTP endpoint alone does not prove
Module delivery. Use the native operations guide's recovery and evidence requirements. Missing
capacity measurements are a warning, not evidence that gameplay is down.
