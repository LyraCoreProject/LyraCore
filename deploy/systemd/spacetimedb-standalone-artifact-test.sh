#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."

unit=deploy/systemd/spacetimedb-standalone.service
runbook=docs/danger-zones.md
fail=0

need() {
  if grep -Eq "$2" "$1"; then
    echo "  ok    $3"
  else
    echo "  FAIL  $3"
    fail=1
  fi
}

echo "[standalone supervisor] unit contract:"
need "$unit" '^Restart=always$' \
  "a clean standalone exit is restarted"
need "$unit" '^RestartSec=[1-9][0-9]*s$' \
  "restart backoff is explicit"
need "$unit" '^LimitNOFILE=524288$' \
  "every standalone process receives the capacity FD limit"
need "$unit" '^StandardError=append:/var/log/lyracore/spacetimedb-standalone\.log$' \
  "standalone stderr is appended to the persistent log"
need "$unit" '^ExecStart=/opt/lyracore/spacetimedb/spacetimedb-standalone start --listen-addr 127\.0\.0\.1:3000 --data-dir /var/lib/lyracore/spacetimedb --non-interactive$' \
  "the pinned standalone binary owns its persistent data directory"
need "$unit" '^User=lyracore$' \
  "standalone runs as the dedicated service account"

echo "[standalone supervisor] runbook contract:"
need "$runbook" 'deploy/systemd/spacetimedb-standalone\.service' \
  "installation names the committed unit"
need "$runbook" 'systemctl daemon-reload' \
  "installation reloads systemd units"
need "$runbook" 'systemctl enable --now spacetimedb-standalone' \
  "installation enables and starts the supervisor"
need "$runbook" 'systemctl status spacetimedb-standalone' \
  "operators can inspect supervisor status"
need "$runbook" 'tail -n 100 /var/log/lyracore/spacetimedb-standalone\.log' \
  "operators can inspect the persistent stderr log"
need "$runbook" 'Live capacity-edge node-death validation' \
  "the production node-death procedure is documented"
need "$runbook" 'approximately 750 seated world sessions' \
  "the capacity-edge seated-session target is explicit"
need "$runbook" '1,000 additional authenticated world logins at \*\*25 per second\*\*' \
  "the queued-login load and admission rate are explicit"
need "$runbook" 'Gateway PID \+ start time before / after \(must match\)' \
  "the runbook proves recovery without a gateway restart"
need "$runbook" 'Verdict: PASS / FAIL / INCONCLUSIVE' \
  "the evidence record cannot silently claim an unrun validation"

if [ "$fail" -eq 0 ]; then
  echo "[standalone supervisor] OK"
else
  echo "[standalone supervisor] FAIL"
  exit 1
fi
