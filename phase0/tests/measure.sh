#!/usr/bin/env bash
# Every number in docs/18-phase-0-report.md, reproduced by one command.
#
#   ./tests/measure.sh              # everything the machine can do
#   SUBSCRIBERS="1000 3000" ./tests/measure.sh
#
# The exit criteria are "stated from evidence, not opinion" (§8, Phase 0), so the evidence is a
# script rather than a paragraph. Results land in phase0/measurements/*.json.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${OUT:-$ROOT/measurements}"
PORT="${PORT:-8099}"
BASE="127.0.0.1:$PORT"
BECK="$ROOT/target/release/beck-p0"
BENCH="$ROOT/target/release/beck-p0-bench"
SUBSCRIBERS="${SUBSCRIBERS:-1000 3000}"
INPROC_SUBSCRIBERS="${INPROC_SUBSCRIBERS:-1000 10000}"
RTTS="${RTTS:-0 25 100}"

mkdir -p "$OUT"
[[ -x "$BECK" && -x "$BENCH" ]] || {
  echo "build first: cargo build --release -p beck-p0-server -p beck-p0-bench"
  exit 1
}

server_pid=""
stop_server() { [[ -n "$server_pid" ]] && kill "$server_pid" 2>/dev/null || true; server_pid=""; }
trap stop_server EXIT

start_server() {
  local store="$1"; shift
  "$BECK" run --store "$store" --addr "$BASE" "$@" >"$OUT/server-$store.log" 2>&1 &
  server_pid=$!
  for _ in $(seq 100); do
    curl -sf "http://$BASE/healthz" >/dev/null && return 0
    sleep 0.2
  done
  echo "server did not start; see $OUT/server-$store.log"
  exit 1
}

echo "==> environment"
{
  echo "{"
  echo "  \"kernel\": \"$(uname -sr)\","
  echo "  \"cpus\": $(nproc),"
  echo "  \"memory_kb\": $(awk '/MemTotal/ {print $2}' /proc/meminfo),"
  echo "  \"rustc\": \"$(rustc --version)\","
  echo "  \"open_file_limit\": $(ulimit -n),"
  echo "  \"postgres\": \"$(psql --version 2>/dev/null || echo absent)\""
  echo "}"
} >"$OUT/environment.json"
cat "$OUT/environment.json"

echo
echo "==> interaction latency (Mode A, memory store, 100 rows in view)"
start_server memory
for rtt in $RTTS; do
  echo "  rtt=${rtt}ms"
  "$BENCH" latency --url "ws://$BASE/socket" --iterations 500 --rows 100 --rtt-ms "$rtt" \
    --json "$OUT/latency-rtt${rtt}.json" >/dev/null
done

echo
echo "==> thin-client payload and first paint"
"$BENCH" payload --http "$BASE" --json "$OUT/payload.json" >/dev/null

echo
echo "==> reconnect-after-deploy resumption"
"$BENCH" resume --url "ws://$BASE/socket" --gap 25 --json "$OUT/resume.json" >/dev/null

echo
echo "==> per-idle-session memory, real websockets"
for n in $SUBSCRIBERS; do
  echo "  subscribers=$n"
  "$BENCH" fanout --url "ws://$BASE/socket" --http "$BASE" --subscribers "$n" --scope mine \
    --json "$OUT/fanout-sockets-$n.json" >/dev/null || {
      echo "  (failed at $n — likely the open-file limit of $(ulimit -n))"
    }
done
stop_server

echo
echo "==> per-idle-session memory, in-process subscriptions"
for n in $INPROC_SUBSCRIBERS; do
  echo "  subscribers=$n"
  "$BENCH" fanout --subscribers "$n" --scope mine --rows 20 --drive 200 \
    --json "$OUT/fanout-inproc-$n.json" >/dev/null
done

echo
echo "==> sequencer throughput and fold replay"
for store in memory redb postgres; do
  if [[ "$store" == postgres ]] && ! psql "${BECK_PG:-postgres://postgres@localhost/beck_p0}" -c 'select 1' >/dev/null 2>&1; then
    echo "  postgres unreachable; skipping"
    continue
  fi
  echo "  store=$store"
  "$BENCH" throughput --store "$store" --clients 32 --commands 20000 \
    --redb-path "$OUT/throughput.redb" --json "$OUT/throughput-$store.json" >/dev/null
done

echo
echo "==> replay determinism (the kill-and-replay property, as a command)"
rm -f "$OUT/verify.redb"
# State determinism is checked over the whole log; the patch stream over a bounded prefix,
# because re-deriving it costs O(events x rows) until Phase 3 makes views incremental.
"$BECK" seed --store redb --redb-path "$OUT/verify.redb" --events 20000 --actors 8 | tee "$OUT/seed.txt"
"$BECK" verify --store redb --redb-path "$OUT/verify.redb" --patch-limit 2000 | tee "$OUT/verify.txt"
"$BECK" replay --store redb --redb-path "$OUT/verify.redb" --genesis | tee "$OUT/replay-genesis.txt"

echo
echo "==> image size (needs apko; skipped otherwise)"
if command -v apko >/dev/null; then
  apko build "$ROOT/deploy/apko/beck-p0.yaml" beck/phase0-todo:measure "$OUT/image.tar" >"$OUT/apko.log" 2>&1
  echo "{\"image_tar_bytes\": $(stat -c%s "$OUT/image.tar")}" >"$OUT/image.json"
else
  echo '{"skipped": "apko is not installed in this environment"}' >"$OUT/image.json"
  echo "  apko absent"
fi

echo
echo "measurements written to $OUT"
ls -1 "$OUT"
