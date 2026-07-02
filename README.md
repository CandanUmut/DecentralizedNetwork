# DecentralizedNetwork · TimeCoin node

A minimal peer-to-peer node, built on **upstream [libp2p](https://libp2p.io) 0.56** (no
forked libraries), that maintains a DAG-based ledger of **TimeCoin** — a coin earned by
recorded contributions, not mining. Nodes discover each other, gossip signed contribution
records into a shared tangle, and **converge on the same DAG and the same balances**.

This is the deliberately small, honest revival of a larger vision (shared compute,
storage, simulations, safe comms). What's in and what's deferred is spelled out in
[SCOPE.md](SCOPE.md); how the old fork-based code was assessed and rebuilt is in
[ASSESSMENT.md](ASSESSMENT.md). The original source is preserved under [`legacy/`](legacy/).

## Quickstart (one machine)

```bash
cargo build --release

# terminal 1 — first node
./target/release/timecoin-node run --data-dir ./data1 --port 9001 --api 127.0.0.1:3001

# terminal 2 — second node, joining the first
./target/release/timecoin-node run --data-dir ./data2 --port 9002 --api 127.0.0.1:3002 \
    --bootstrap /ip4/127.0.0.1/tcp/9001
```

(On the same LAN, mDNS finds peers automatically — `--bootstrap` is only required across
networks.) Then record a contribution and watch it converge:

```bash
# mint 100 TimeCoin to node 2's wallet for a contribution
./target/release/timecoin-node reward --api 127.0.0.1:3002 --amount 100 --memo "wrote the docs"

# send 40 to node 1 (peer id or 0x wallet both work)
./target/release/timecoin-node send --api 127.0.0.1:3002 --to <node1-peer-id> --amount 40

# ask *either* node — the answers match
./target/release/timecoin-node status   --api 127.0.0.1:3001
./target/release/timecoin-node balances --api 127.0.0.1:3001
```

`status` shows connected peers, DAG size, tip count, your wallet and balance, and your
full listen addresses (the things you give to friends).

## One-command local cluster

```bash
docker compose up --build
```

Three isolated nodes come up with APIs on `localhost:3001..3003`:

```bash
curl -s localhost:3002/reward -H 'content-type: application/json' \
     -d '{"amount":100,"memo":"first contribution"}'
curl -s localhost:3001/balances   # same answer from any node
```

## How a friend on another machine joins your network

1. **You** start a node and read your address from `status`:

   ```bash
   timecoin-node run --data-dir ./data --port 9000
   timecoin-node status | grep -A6 listen_addrs
   # e.g. /ip4/203.0.113.7/tcp/9000/p2p/12D3KooW...
   ```

   Give your friend an address with your **public** IP (or DNS name). If you're behind a
   router, forward **TCP+UDP 9000** to your machine (or let UPnP do it — the node asks
   your router automatically and logs `UPnP external address` when it works).

2. **Your friend** runs:

   ```bash
   timecoin-node run --data-dir ./data --port 9000 \
       --bootstrap /ip4/YOUR.PUBLIC.IP/tcp/9000/p2p/YOUR_PEER_ID
   ```

   On connect, the nodes exchange full DAGs (anti-entropy sync), then stay in sync via
   gossip. `status` on both sides should show the same `dag_transactions` within seconds.

3. **If neither side is reachable** (both behind NAT, no port forwarding): run a third
   node somewhere reachable (any cheap VPS; every node is also a relay) and have NATed
   nodes reserve a slot on it:

   ```bash
   # on the VPS (tell it its own public address so reservations carry it)
   timecoin-node run --data-dir ./data --port 9000 \
       --external-address /ip4/VPS.IP/tcp/9000
   # on each NATed machine
   timecoin-node run --data-dir ./data --port 9000 \
       --bootstrap /ip4/VPS.IP/tcp/9000/p2p/VPS_PEER_ID \
       --relay     /ip4/VPS.IP/tcp/9000/p2p/VPS_PEER_ID
   ```

   NATed nodes then advertise a `…/p2p-circuit/p2p/<their-id>` address (shown in
   `status` under `listen_addrs`) that anyone can `--bootstrap` to; connections start
   through the relay, and DCUtR attempts to upgrade them to direct hole-punched
   connections. AutoNAT tells each node whether it's publicly reachable. This
   reserve-then-dial-through-the-circuit flow is exercised by `scripts/relay-demo.sh`.

**NAT honesty:** UPnP fails on many routers and all CGNAT; hole punching fails on
symmetric NATs. The relay path always works but routes traffic through the relay. There
are no default public bootstrap/relay nodes for this network yet — someone in your group
must run one reachable node.

## HTTP API

| Endpoint | Method | What |
|---|---|---|
| `/status` | GET | peer id, wallet, balance, peers, DAG size/tips, listen addrs |
| `/peers`, `/tips`, `/transactions`, `/balances`, `/balance` | GET | the ledger, from this node's view |
| `/reward` | POST | `{"wallet?": "0x…", "amount": 100, "memo": "what was contributed"}` — mint for a contribution |
| `/send` | POST | `{"to": "0x… or peer id", "amount": 40}` — transfer |
| `/connect` | POST | `{"address": "/ip4/…/tcp/…/p2p/…"}` — dial a peer |

The API binds to `127.0.0.1` by default: anyone who can reach it can spend from the
node's wallet. Only expose it on trusted networks (the docker cluster binds `0.0.0.0`
inside the compose network for the demo).

## What works / what doesn't

**Works (exercised in this repo):**
- Persistent peer identity; stable peer id + wallet across restarts.
- Encrypted, authenticated transport (Noise or TLS 1.3, standard libp2p) over TCP and QUIC.
- Discovery: mDNS locally, explicit bootstrap addresses remotely.
- Signed transactions gossiped via gossipsub; full-DAG sync on connect; orphaned
  transactions re-request their missing parents; DAG persisted per node.
- Verified: 3-node cluster + late joiner all converge to identical DAG, tips, and
  balances; docker-compose cluster verified the same way.
- Relay fallback verified end to end (`scripts/relay-demo.sh`, with mDNS disabled): a
  node reserves a circuit slot on a relay, and a third node reaches it by dialing only
  the circuit address, then syncs the DAG through it.
- AutoNAT, DCUtR, and UPnP are wired from upstream crates, but **hole punching across
  real-world NATs has not been field-tested from this environment**. Expect to lean on
  the relay fallback first.

**Doesn't work / doesn't exist (on purpose, for now):**
- **No consensus, no Sybil resistance, no spam control: any node can mint TimeCoin with
  `/reward` and the network accepts it.** Balances can go negative if a modified client
  overspends — honest nodes refuse to *create* such transfers, but nothing rejects them
  network-wide. This is a ledger for *cooperating* peers, not money for adversarial ones.
- No IPFS/file storage, WebRTC, browser nodes, DHT discovery, social layer, or compute
  marketplace (see SCOPE.md for why and what it would take).
- Full-DAG sync sends everything to every new peer — fine for demos, not for big ledgers.
- **Not audited.** Do not use for anything valuable.

## What I'd do next

1. Reject-or-quarantine rules for overdraft branches, so a bad client can't push
   balances negative network-wide (per-wallet sequence numbers or ancestry-funded checks).
2. Replace full-DAG sync with tip-based delta sync (exchange tips, walk back only the
   missing ancestry).
3. A shared, well-known bootstrap+relay node and a config file for a "community" of nodes.
4. Rate limits / proof-of-contribution rules on `reward` — the actual economics.
5. Kademlia DHT under a `/timecoin/kad/1.0.0` protocol for discovery beyond bootstrap lists.
6. A tiny read-only web dashboard on top of the existing JSON API.
7. Bring back content storage via a maintained IPFS implementation as a normal dependency.

## Repo layout

```
src/            the node (dag, network, api, identity, cli)
legacy/         the original fork-era source, unmodified, for reference
ASSESSMENT.md   Phase-0 inventory: what was here, what the fork did, what was salvaged
SCOPE.md        what this MVP is and is not
CHANGELOG.md    what changed and why
```

## License

MIT (see [LICENSE](LICENSE)).
