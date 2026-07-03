#!/usr/bin/env bash
# Demonstrates the relay fallback on one machine:
#   relay  <- a publicly reachable node (here: localhost with --external-address)
#   natted <- reserves a circuit slot on the relay
#   friend <- reaches `natted` by dialing ONLY the relay circuit address,
#             then syncs the DAG through it.
set -euo pipefail
cd "$(dirname "$0")/.."

BIN=./target/release/timecoin-node
[ -x "$BIN" ] || cargo build --release

DIR=$(mktemp -d)
trap 'kill $(jobs -p) 2>/dev/null; rm -rf "$DIR"' EXIT
jqget() { python3 -c "import json,sys; d=json.load(sys.stdin); print($1)"; }

echo "== starting relay"
$BIN run --no-mdns --data-dir "$DIR/relay" --port 9301 --api 127.0.0.1:3301 \
    --external-address /ip4/127.0.0.1/tcp/9301 >"$DIR/relay.log" 2>&1 &
sleep 2
RELAY_ID=$($BIN status --api 127.0.0.1:3301 | jqget "d['peer_id']")
RELAY_ADDR=/ip4/127.0.0.1/tcp/9301/p2p/$RELAY_ID

echo "== starting NATed node, reserving a slot on the relay"
$BIN run --no-mdns --data-dir "$DIR/natted" --port 9302 --api 127.0.0.1:3302 \
    --bootstrap "$RELAY_ADDR" --relay "$RELAY_ADDR" >"$DIR/natted.log" 2>&1 &
sleep 3

echo "== relay attests a contribution by the NATed node (rewards are attestations by others)"
NATTED_ID=$($BIN status --api 127.0.0.1:3302 | jqget "d['peer_id']")
$BIN reward --api 127.0.0.1:3301 --to "$NATTED_ID" --amount 77 --memo "made behind NAT" >/dev/null

CIRCUIT=$($BIN status --api 127.0.0.1:3302 \
    | jqget "[a for a in d['listen_addrs'] if 'p2p-circuit' in a][0]")
echo "   circuit address: $CIRCUIT"

echo "== friend dials ONLY the circuit address"
$BIN run --no-mdns --data-dir "$DIR/friend" --port 9303 --api 127.0.0.1:3303 \
    --bootstrap "$CIRCUIT" >"$DIR/friend.log" 2>&1 &
sleep 4

TXS=$($BIN status --api 127.0.0.1:3303 | jqget "d['dag_transactions']")
echo "== friend sees $TXS transactions (expected 3: 2 genesis + 1 reward)"
[ "$TXS" = "3" ] && echo "RELAY DEMO OK" || { echo "RELAY DEMO FAILED"; exit 1; }
