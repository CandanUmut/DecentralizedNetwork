# DecentralizedNetwork · TimeCoin node

[![CI](https://github.com/CandanUmut/DecentralizedNetwork/actions/workflows/ci.yml/badge.svg)](https://github.com/CandanUmut/DecentralizedNetwork/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE)

A community's own network: every member's computer keeps one shared, tamper-proof record
of **who helped whom**. **TimeCoin** is created only by *thanking* someone — never bought
or mined — and each member's thanks can create only a **fixed daily allowance**, agreed
at the community's birth. Members message each other end-to-end encrypted and pay each
other to store data. No company, no server, no ads. **Read [VISION.md](VISION.md) for
the why** — built on upstream [libp2p](https://libp2p.io) 0.56, no forked libraries.

This is the deliberately small, honest core of a larger vision (shared compute, storage,
simulations, safe comms + earning). **How it all works under the hood — the DAG, the
libp2p networking, discovery without servers, the security model, the future compute
design: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).** Where it's going:
[docs/ROADMAP.md](docs/ROADMAP.md). How this project was revived from its original code
— and that code itself — lives in [docs/history/](docs/history/) and in git history at
commit `2192a1b` (tagged `legacy-source` locally).

## The ledger rules (30 seconds)

- Your **wallet** is derived from your node's persistent key; your balance is derived
  from the shared DAG, never stored as a mutable counter.
- **Rewards are attestations.** You cannot mint to yourself; you mint to the wallet of
  someone who provided value to you, and your wallet is on record as the voucher.
- **Transfers carry sequence numbers.** Balances come from a deterministic fold that
  every node computes identically: double-spends resolve to the same winner everywhere,
  and an overdrafting transaction — even from a modified client — simply never applies.
  Network-wide balances cannot go negative.
- **Giving is rationed like time.** Each member's thanks can create at most the
  community's `--daily-allowance` (default 100 TC) per day — enforced by every node's
  ledger fold, with timestamps that must flow forward along the DAG and future-dated
  transactions parked until their time arrives. That scarcity is the economy.
- **Trust is social.** You `vouch` for wallets you know (revocably); your *trusted view*
  counts only rewards minted inside your vouch neighborhood, and a member's rewards only
  count from the moment someone trusted vouched for them — no backdated histories.
- Communities are isolated: the network name **and its economy rules** are baked into
  genesis; joining one is a single join file (or a scanned QR that opens an invite page).

## Install

```bash
git clone https://github.com/CandanUmut/DecentralizedNetwork
cd DecentralizedNetwork && ./install.sh     # builds and installs to ~/.local/bin
```

Tagged releases also ship prebuilt Linux/macOS/Windows binaries via GitHub Actions
(see the Releases page once a `v*` tag is pushed). Then:

```bash
timecoin-node run --data-dir ~/.timecoin
# open http://127.0.0.1:3000 — the dashboard is phone-friendly:
# Home (balances/activity) · Act (thank/send/vouch + daily allowance meter)
# Chat (encrypted messages, by name) · Join (QR invite)
```

The QR on the Join tab opens a read-only **invite page** (served on `--invite`, default
`0.0.0.0:3080`) that explains the project and hands your friend the join file — scanning
it on a phone shows a real page, not raw JSON. `--invite off` disables it.

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

# web of trust: set a name, vouch for people you know, view trusted balances
tc name  --api 127.0.0.1:3001 --set umut
tc vouch --api 127.0.0.1:3001 --to $NODE2
tc revoke --api 127.0.0.1:3001 --to $NODE2   # people fall out; trust is revocable
tc trust --api 127.0.0.1:3001
tc balances --api 127.0.0.1:3001 --trusted   # only vouched-for minters count

# spot-check that a provider still holds what you paid it to store
tc verify --api 127.0.0.1:3002 --peer $NODE1 --hash <hash from store>

# invite a friend: hand them one file
tc join-file --api 127.0.0.1:3001 > my-community.json
# friend runs: tc run --data-dir ./data --config my-community.json

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
| `/vouch` | POST | `{"to": "wallet or peer id"}` — on-ledger statement of trust |
| `/trust?depth=3` | GET | your vouch neighborhood; `?trusted=true` on `/balance(s)` uses it |
| `/name` POST, `/names` GET | | display names (labels, not unique identities) |
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
- Discovery: mDNS locally (`--no-mdns` to disable), explicit bootstrap remotely, and a
  **Kademlia DHT** under the community's own protocol id — peers-of-peers are dialable
  by id (verified: a node messaged another it had never connected to).
- Attested rewards (self-mint rejected), sequenced transfers, deterministic double-spend
  resolution and overdraft immunity (unit-tested; convergence verified across live nodes).
- Web of trust with **revocation**; trusted balance views with optional per-attester
  **mint caps** (blast-radius limit for a rogue voucher).
- **Trust-gated storage:** providers can refuse anyone outside their vouch neighborhood
  (at quote time, before payment) and verify payments against their *trusted* ledger
  view — rigged coin from outside buys nothing. Verified live, including refusal paths.
- Custody **spot-checks**: store-time probes let you later verify a provider still holds
  your blob (verified live: deletion is detected).
- **Daily giving allowance** enforced identically by every node (verified live: an
  over-allowance thank is refused locally with the remaining budget, and even a modified
  client's excess mint counts for zero on all nodes); allowance meter in the app.
- **Self-healing connections**: a node that finds itself alone redials its bootstrap
  peers every 30 s (verified live across a peer restart).
- Direct messages (shown by *name*), delta sync, relay fallback, one-command docker
  cluster, mobile-first dashboard, QR → invite page → join file flow.

**Doesn't work / doesn't exist (on purpose, for now — see [docs/ROADMAP.md](docs/ROADMAP.md)):**
- **Sybil resistance is social, not cryptographic.** Trusted views + trust-gated
  services + mint caps make rigged coin useless *inside* a well-kept neighborhood, but
  nothing stops strangers from minting in the raw view, and a vouched wallet that goes
  rogue can still act within its cap until revoked. Keep networks private to people you
  trust; vouch deliberately.
- **Custody checks are spot-checks, not proofs:** a failed check tells you to stop
  trusting a provider; it doesn't refund you (no escrow/slashing yet — Stage 4). A
  payment burned on a refused Put after a passed quote is also not auto-refunded.
- No store-and-forward messaging for offline peers.
- Full history still replicates to every node; blobs capped (512 KiB default).
- **Known allowance edge:** a member can back-fill *unused past days* of their own
  allowance by attaching to old parents with old dates. It's visible on-chain as
  backdating, bounded by their membership anchor in trusted views, and will be closed by
  periodic checkpoints (ROADMAP). It does not allow exceeding any day's budget.
- **Not audited.** Don't put anything valuable on it.

## Repo layout

```
src/             the whole node: dag.rs (ledger rules), network.rs (libp2p),
                 api.rs (HTTP + orchestration), main.rs (CLI), *.html (web UI)
scripts/         runnable demos (relay fallback)
docs/ROADMAP.md  the staged path from here to the full vision, open problems included
docs/history/    how this came to be: assessment of the original code, scope
                 decisions, the author's original notes (pre-rewrite source
                 itself is in git history at commit `2192a1b` (tagged `legacy-source` locally))
VISION.md        why this exists — start here if you're new
CONTRIBUTING.md  build/test/demo instructions and the ground rules
CHANGELOG.md     what changed and why, release by release
```

## Troubleshooting

- **"rejected transaction … unknown genesis / orphan pool full" floods** — an old-version
  node (leftover process, docker container, or another machine on your LAN with an old
  data dir) is talking to you. Since v0.5.2 incompatible versions can't exchange
  transactions at all; to clean your machine: `pkill timecoin-node`,
  `docker compose down`, delete old `--data-dir` directories, rebuild, start fresh.
- **Upgrading across versions that changed ledger rules** (see CHANGELOG "breaking"
  notes): all members must upgrade together and start fresh data dirs — the economy
  rules are part of the network's identity, so old and new rules are different networks
  by design.
- **Two nodes on one machine** — give each its own `--data-dir`, `--port`, `--api`, and
  either distinct `--invite` ports or `--invite off`.

## Contributing

PRs welcome — read [CONTRIBUTING.md](CONTRIBUTING.md) first (two-minute read: how to
build and test, where things live, and the ground rules — chiefly: never fake, and
determinism in the ledger is sacred). Security reports: see
[.github/SECURITY.md](.github/SECURITY.md).

## License

MIT (see [LICENSE](LICENSE)).
