# Movement batch acceptance

This non-production check measures the existing steady-heartbeat batching path. It does not publish,
seed, or select a database. The command refuses any realm name that does not start with
`disposable:`. The selected load driver owns environment setup and must independently refuse unsafe
targets.

Use the pinned wire harness, or another approved synthetic driver, to seat at least 500 active
movers. The driver must write these `key=value` lines after one measurement window:

- `seated_movers`, `submitted_entries`, `reducer_calls`, `failed_entries`, and `failed_calls`
- `batch_size_buckets`, using the gateway buckets `1`, `2-32`, `33-64`, `65-127`, and `128`
- `transaction_p95_ms` and `action_latency_p95_ms`, measured during that same window

The gateway's cumulative `MOVEBATCH` observation provides the movement values. Take a snapshot before
and after the run and give the driver-produced deltas to the command. Empty intervals have zero calls
and report `batch_factor=NA`. Do not turn that into zero or infinity.

```sh
scripts/movement-batch-acceptance.sh \
  --realm disposable:movement-acceptance \
  --load-driver /absolute/path/to/approved-driver \
  --movers 500 \
  --duration 120 \
  --visual-verdict NOT_RUN
```

The summary is accepted only with at least 500 seated movers, one or more reducer calls, and no failed
calls or entries. It always reports both batch-factor operands, the bounded size distribution, the
transaction-duration signal, and the action-latency signal. A lower transaction count alone is not a
pass.

The measurement tripwire is action latency or transaction duration getting worse while the batch
factor improves. When it fires, adjust levers in this order: mover heartbeat rate, the 40 ms drain
cadence, then the 128-entry call cap. Measure after each single change. Do not introduce adaptive
cadence without a separate design and acceptance run.

## Manual client check

Automation cannot claim a real-client visual verdict. During the same configuration, use an
unmodified 1.12.1 client to observe nearby movers, state edges, and a representative action. Record
`PASS`, `FAIL`, or `NOT_RUN` through `--visual-verdict`; `NOT_RUN` keeps the automated measurement
honest and leaves the human gate explicit.
