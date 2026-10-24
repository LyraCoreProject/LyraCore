# Update a native Realm

Use this branch after the user authorizes an update to the named host. Read the
[production contract](production-contract.md) and follow
[the native operations guide](../../../../docs/native-operations.md).

1. Identify the approved target, current and intended commits, Native Host service, data directory,
   private publisher configuration, explicit loopback HTTP URI and Shard. Name adjacent Realms that
   are outside the operation. Resolve a missing target before any write. Existing authorization for
   these exact values does not require another confirmation.
2. Preserve tracked edits. Prepare the intended revision and installed Packages, complete the
   relevant tests and `./lyracore preflight`, and retain their output. Do not publish after a failed
   prerequisite or critical skipped check.
3. Take a consistent backup of the data, matching executable, Module, keys and configuration.
   Restore a copy into a separate directory on unused loopback ports and verify it as the native
   operations guide specifies. Keep credentials on the target machine.
4. Publish with `./lyracore publish --server <http-uri> <shard>`. Do not clear a Shard or use the
   retired multi-Shard publication command. Apply the change's explicit migration requirements with
   the same publisher identity.
5. If the Host changed, install the tested executable and restart only the identified native
   service. Coordinate Host and Module interface changes with client listeners closed. Preserve
   the configured data directory, keys, publisher, Shard and listener addresses.
6. Verify the new process, sockets, Module hash, native authentication, world entry, subsequent
   requests and durable state. For recovery or load work, report duration, activity, latency and
   resource use. Capture real-client evidence when the behavior requires it.

A failed publish or migration blocks the next step. Preserve the failure evidence and the current
service state. Diagnose within the authorized target; do not expand into another Realm, clear data
or restore a backup that would discard later writes without the Operator's decision.

Report every gate with the skill's result contract. Include the before/after commits, artifact
hashes, backup location, service state, passing checks, unresolved warnings and the smallest next
step. No Gateway service should be present in the native topology.
