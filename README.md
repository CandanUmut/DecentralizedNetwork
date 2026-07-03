# DecentralizedNetwork · TimeCoin node

A minimal peer-to-peer node, built on **upstream [libp2p](https://libp2p.io) 0.56** (no
forked libraries), that maintains a DAG-based ledger of **TimeCoin** — a coin earned by
attested contributions, not mining. Nodes discover each other, gossip signed records into
a shared tangle, **converge on the same ledger and balances**, exchange encrypted direct
messages, and can **pay each other to store data**.

This is the deliberately small, honest core of a larger vision (shared compute, storage,
simulations, safe comms + earning). Where it's going: [ROADMAP.md](ROADMAP.md). What's in
vs. deferred: [SCOPE.md](SCOPE.md). How the old fork-based code was assessed and rebuilt:
[ASSESSMENT.md](ASSESSMENT.md). The original source is preserved under [`legacy/`](legacy/).

## The ledger rules (30 seconds)

- Your **wallet** is derived from your node's persistent key; your balance is derived
  from the shared DAG, never stored as a mutable counter.
- **Rewards are attestations.** You cannot mint to yourself; you mint to the wallet of
  someone who provided value to you, and your wallet is on record as the voucher.
- **Transfers carry sequence numbers.** Balances come from a deterministic fold that
  every node computes identically: double-spends resolve to the same winner everywhere,
  and an overdrafting transaction — even from a modified client — simply never applies.
  Network-wide balances cannot go negative.
- Communities are isolated: `--network koop` has its own genesis, topics, and protocols.

## Quickstart (one machine)

```bash
cargo build --release
alias tc=./target/release/timecoin-node

# terminal 1 — first node (dashboard at http://127.0.0.1:3001)
tc run --data-dir ./data1 --port 9001 --api 127.0.0.1:3001

# terminal 2 — second node, joining the first
tc run --data-dir ./data2 --port 9002 --api 127.0.0.1:3002 \
    --bootstrap /ip4/127.0.0.1/tcp/9001
```

(On the same LAN, mDNS finds peers automatically — `--bootstrap` is only required across
networks.) Then use it — everything below converges to both nodes within a second:

```bash
NODE1=$(tc status --api 127.0.0.1:3001 | jq -r .peer_id)
NODE2=$(tc status --api 127.0.0.1:3002 | jq -r .peer_id)

# node 1 attests that node 2's owner contributed (mints 100 to them)
tc reward --api 127.0.0.1:3001 --to $NODE2 --amount 100 --memo "fixed my bike"

# node 2 pays 40 back
tc send --api 127.0.0.1:3002 --to $NODE1 --amount 40

# encrypted direct message
tc message --api 127.0.0.1:3001 --to $NODE2 --text "tesekkurler!"
tc inbox   --api 127.0.0.1:3002

# pay node 1 to store data (1 TC per started 100 KiB by default)
tc store --api 127.0.0.1:3002 --peer $NODE1 --text "important note"
tc fetch --api 127.0.0.1:3002 --peer $NODE1 --hash <hash from store>

# same answers from any node
tc balances --api 127.0.0.1:3001
tc status   --api 127.0.0.1:3002
```

Open `http://127.0.0.1:3001` in a browser for the live dashboard (peers, tips, balances,
transaction feed with applied/not-applied state).

## One-command local cluster

```bash
docker compose up --build
```

Three isolated nodes with APIs + dashboards on `localhost:3001..3003` (see the comment
header in `docker-compose.yml` for a copy-paste demo).

## How a friend on another machine joins your network

1. **You** start a node and read your address from `status`:

   ```bash
   tc run --data-dir ./data --port 9000
   tc status | jq .listen_addrs
   # e.g. /ip4/203.0.113.7/tcp/9000/p2p/12D3KooW...
   ```

   Give your friend an address with your **public** IP (or DNS name). If you're behind a
   router, forward **TCP+UDP 9000** to your machine (or let UPnP do it — the node asks
   your router automatically and logs `UPnP external address` when it works).

2. **Your friend** runs:

   ```bash
   tc run --data-dir ./data --port 9000 \
       --bootstrap /ip4/YOUR.PUBLIC.IP/tcp/9000/p2p/YOUR_PEER_ID
   ```

   On connect, the nodes exchange DAG tips and pull missing history (delta sync), then
   stay in sync via gossip. `status` on both sides shows the same `dag_transactions`
   within seconds.

3. **If neither side is reachable** (both behind NAT, no port forwarding): run a third
   node somewhere reachable (any cheap VPS; every node is also a relay) and have NATed
   nodes reserve a slot on it:

   ```bash
   # on the VPS (tell it its own public address so reservations carry it)
   tc run --data-dir ./data --port 9000 \
       --external-address /ip4/VPS.IP/tcp/9000
   # on each NATed machine
   tc run --data-dir ./data --port 9000 \
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
| `/` | GET | live dashboard (HTML) |
| `/status` | GET | peer id, wallet, balance, peers, DAG size/tips, inbox count, listen addrs |
| `/peers`, `/tips`, `/transactions`, `/balances`, `/balance` | GET | the ledger, from this node's view (`/transactions` flags each transfer `applied` or not) |
| `/reward` | POST | `{"to": "wallet or peer id", "amount": 100, "memo": "…"}` — attest someone else's contribution |
| `/send` | POST | `{"to": "wallet or peer id", "amount": 40}` — transfer |
| `/message` | POST | `{"peer_id": "…", "text": "…"}` — encrypted direct message |
| `/messages` | GET | this node's inbox |
| `/store` | POST | `{"peer_id": "…", "text": "…"}` or `{"peer_id": "…", "data_base64": "…"}` — quote, pay, store |
| `/fetch/{hash}?peer=…` | GET | fetch a blob; content hash verified locally |
| `/connect` | POST | `{"address": "/ip4/…/tcp/…/p2p/…"}` — dial a peer |

The API binds to `127.0.0.1` by default: anyone who can reach it can spend from the
node's wallet. Only expose it on trusted networks (the docker cluster binds `0.0.0.0`
inside the compose network for the demo).

## What works / what doesn't

**Works (exercised in this repo, live, multi-node):**
- Persistent peer identity; stable peer id + wallet + ledger across restarts.
- Encrypted, authenticated transport (Noise / TLS 1.3, standard libp2p) over TCP and QUIC.
- Discovery: mDNS locally (`--no-mdns` to disable), explicit bootstrap addresses remotely.
- Attested rewards (self-mint rejected), sequenced transfers, deterministic double-spend
  resolution and overdraft immunity (unit-tested; convergence verified across live nodes).
- Delta sync on connect (tips + batched ancestry pulls) with orphan re-request.
- Direct messages between connected peers, with peer addresses learned via identify.
- Paid storage: quote → ledger payment (verified as *applied*, anti-double-redeem,
  persisted across provider restarts) → store → fetch with local hash verification.
- Relay fallback verified end to end (`scripts/relay-demo.sh`); one-command docker cluster.

**Doesn't work / doesn't exist (on purpose, for now — see ROADMAP.md):**
- **No Sybil resistance:** one human with two keys can attest themselves. Rewards are
  attributable (you can see who vouched), but trust weighting is Stage 3. Keep networks
  private to people you trust.
- **No proof of continued storage:** a paid provider could delete your blob later and
  keep the payment; there's no challenge protocol or escrow yet (Stage 4). Also, if a
  provider refuses after taking payment, the payment is not automatically refunded.
- Messaging needs an existing connection (or a dialable/circuit address) — there's no
  DHT-based peer lookup yet, and no store-and-forward for offline peers.
- Full history still replicates to every node eventually; blobs are capped (512 KiB
  default) by the wire codec's limits.
- **Not audited.** Don't put anything valuable on it.

## Repo layout

```
src/            the node (dag, network, api, identity, dashboard, cli)
scripts/        runnable demos (relay fallback)
legacy/         the original fork-era source, unmodified, for reference
ASSESSMENT.md   Phase-0 inventory: what was here, what the fork did, what was salvaged
SCOPE.md        what the revival MVP was and was not
ROADMAP.md      the staged path from here to the full vision
CHANGELOG.md    what changed and why
```

## License

MIT (see [LICENSE](LICENSE)).
