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
- **Trust is social.** You `vouch` for wallets you know; your *trusted view* of balances
  counts only rewards minted inside your vouch neighborhood, so a stranger's self-minted
  fortune is worth nothing to you. Wallets can set display names (labels, not identities).
- Communities are isolated: `--network koop` has its own genesis, topics, and protocols —
  and joining one is a single `join-file` handed to a friend.

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
# Home (balances/activity) · Act (thank/send/vouch) · Chat · Join (QR invite)
```

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
- Direct messages, delta sync, relay fallback, one-command docker cluster, mobile-first
  dashboard with QR invites.

**Doesn't work / doesn't exist (on purpose, for now — see ROADMAP.md):**
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
