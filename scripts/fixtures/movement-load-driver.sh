#!/bin/sh
# Deterministic fixture for movement-batch-acceptance-test.sh. A real driver emits this same contract.
printf '%s\n' \
  'seated_movers=@SEATED@' \
  'submitted_entries=@ENTRIES@' \
  'reducer_calls=@CALLS@' \
  'failed_entries=@FAILED_ENTRIES@' \
  'failed_calls=@FAILED_CALLS@' \
  'batch_size_buckets=1:0,2-32:0,33-64:0,65-127:4,128:96' \
  'transaction_p95_ms=5.4' \
  'action_latency_p95_ms=18.2'
