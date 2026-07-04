# CHANGELOG

## 0.4.0 — 2026-07-04 · Trust-gated economy, DHT, custody checks, phone UI

**Mint security & trust (user-requested hardening)**
- **Vouch revocation:** `Revoke` transaction; for each (voucher, vouchee)
  pair the latest statement wins, and revoking cuts the whole subtree out
  of your trust neighborhood. `revoke` CLI + `/revoke`.
- **Mint blast-radius caps:** trusted views accept `mint_cap` — a limit on
  how much total mint any single attester contributes (deterministic:
  rewards considered in (timestamp, id) order). A rogue vouched wallet can
  no longer inflate your view without bound.
- **Trust-gated storage:** `--blob-trust-depth N` makes a provider refuse
  storage requests from wallets outside its vouch neighborhood *at quote
  time* (before any payment), and verify payments against its **trusted**
  ledger view — coins minted outside the neighborhood buy nothing.
  Verified live end to end, including the refusal paths.

**Network protocols**
- **Kademlia DHT** under `/timecoin/<net>/kad/1.0.0` (server mode on every
  node): identify/mDNS feed the routing table, bootstrap runs on connect
  and every 5 minutes. Peers-of-peers become dialable by id — verified
  live: a node messaged another it had never connected to.

**Storage custody (Stage 4 seed)**
- At store time the client saves 4 random byte-range probes (hash of each
  slice); `verify --peer --hash` (or `/verify/{hash}?peer=`) asks the
  provider for one range and compares. A provider that deleted the blob
  fails the check. Verified live both ways. (Spot-check, not a proof
  system: no escrow/slashing yet — see ROADMAP Stage 4.)

**Phone-friendly face**
- The dashboard is now a mobile-first app: Home (trusted-first balances,
  activity, peers), Act (thank/send/vouch/revoke/set-name forms with
  name autocomplete), Chat (send + inbox), Join (**QR code** of the
  community join file, rendered server-side at `/join-qr.svg`).
- Inbox messages now carry the sender's wallet; names resolve everywhere.

**Install & distribution**
- `install.sh` (build + install to ~/.local/bin with quickstart).
- GitHub Actions: CI (tests + clippy -D warnings) on every push; tagged
  releases build Linux/macOS/Windows binaries automatically.

**Fixes**
- Custody probe generation panicked on blobs < 16 bytes.
- Second genesis transaction was excluded from trusted views.
- CLI `get` now surfaces the node's JSON error bodies like `post` does.


## 0.3.0 — 2026-07-03 · A network of people (ROADMAP Stage 3 core)

- **Web of trust:** new `Vouch` transaction — a signed on-ledger statement
  that you know and trust a wallet (self-vouching invalid). `trust` / `/trust`
  shows your neighborhood (BFS over vouch edges, `--depth`, default 3).
  **Trusted balance views** (`balances --trusted`, `/balances?trusted=true`)
  recompute the ledger counting only rewards minted by wallets within your
  vouch horizon: a stranger's self-minted fortune is plainly visible in the
  raw view and worth zero in yours. Verified live: stranger minted 5000 to a
  wallet; raw view 5100, trusted view 100.
- **Display names:** `Profile` transaction sets a wallet's name (`name --set`,
  1–32 printable chars, latest wins). Deliberately non-unique labels — the
  wallet stays the identity (first-claim uniqueness on a DAG is
  grind-vulnerable; documented in ROADMAP). Dashboard and API show names.
- **One-file community join:** `join-file` emits `{network, bootstrap, relay}`
  from a running node; `run --config file.json` joins from it. Verified live
  round-trip. `/status` now reports the network name; dashboard shows it and
  a trusted-balance tile.
- Mint budgets / rate limits remain an open design problem (self-claimed
  timestamps vs. deterministic epochs) — recorded honestly in ROADMAP.md
  rather than faked.


## 0.2.0 — 2026-07-02 · Trustworthy ledger + first services (ROADMAP Stages 1–2)

**Ledger v2** (breaking: new tx format, protocols renamed; start fresh data dirs)
- **Attested rewards:** minting to your own wallet is invalid; a reward is a
  signed statement that *someone else* provided value, with the voucher's
  wallet on record. `reward` now takes `--to`.
- **Sequenced transfers + deterministic fold:** transfers carry per-sender
  sequence numbers; balances are computed by a fixpoint fold that applies
  transfers in (sender, seq, lowest-hash) order only when funded. Double
  spends resolve to the same winner on every node; overdrafts from modified
  clients are structurally accepted but never applied — network-wide
  balances cannot go negative. `/transactions` now flags `applied`.
- **Named networks:** `--network <name>` derives distinct genesis, gossip
  topic, and protocol ids per community.
- **Delta sync:** peers exchange DAG tips on connect and pull missing
  ancestry in batched rounds (512 txs/response) instead of full id lists.
  Orphan pool capped (10k) against memory-exhaustion floods.

**Services (ROADMAP Stage 2)**
- **Direct messaging** (`/timecoin/<net>/msg/1.0.0`): encrypted,
  peer-authenticated text messages between connected peers; inbox in
  `/messages`, CLI `message`/`inbox`. Peer addresses learned from identify
  so a once-met peer can be redialed.
- **Paid blob storage** (`/timecoin/<net>/blob/1.0.0`): quote → pay on the
  ledger → store → fetch. Providers verify the payment transaction is a
  transfer to their wallet, of at least the quoted price, **applied** by the
  ledger fold (a double-spend loser can't buy storage), and never redeemed
  before (redemptions persisted). Content fetched is hash-verified by the
  client. `--blob-price` / `--blob-max-kib`; price 0 = store for free.
- **Dashboard:** `GET /` serves a live HTML dashboard (peers, tips,
  balances, transaction feed with applied state).
- ROADMAP.md: the staged plan from here to the shared compute/storage
  vision, including what is deliberately not built yet and why.

**Verified live:** 3 nodes + late joiner converge (attested reward,
transfer, storage payment; identical balances everywhere); self-mint
rejected over the API; message delivery both ways; paid store/fetch with
the provider earning 1 TC; restart keeps identity, ledger, and redeemed
payments; relay demo still green.


## 0.1.0 — 2026-07-02 · Revival: fork removed, MVP node rebuilt

**Phase 0 — inventory (no code changes)**
- Added `ASSESSMENT.md`: the repo contained application source only — no
  `Cargo.toml`/`Cargo.lock`, and the modified libp2p / rust-ipfs checkouts the
  code was built against were never pushed. Every inferred fork customization
  is cataloged there with an (A)/(B)/(C)/(D) tag.
- Added `SCOPE.md`: MVP vs. deferred (IPFS storage, WebRTC, STUN/TURN,
  public-IPFS-DHT, social layer, compute marketplace).

**Phase 1 — de-fork & modernize**
- Moved the old source to `legacy/` (kept verbatim as the design reference).
- New `timecoin-node` crate pinned to **upstream libp2p 0.56** — no forked
  dependencies anywhere. Fork-era behavior re-expressed as our own code:
  - `/messaging/1.0.0` → `/timecoin/sync/1.0.0` (request_response + CBOR),
  - hand-rolled keep-alive `ConnectionHandler` → `with_idle_connection_timeout`,
  - hand-assembled `OrTransport` stack → `SwarmBuilder` (TCP + QUIC + DNS,
    Noise, Yamux),
  - STUN/TURN/WebRTC/UPnP side stack → upstream autonat + relay + dcutr + upnp
    behaviours (the old TURN credentials were hardcoded in source; treated as
    leaked and removed).

**Phase 2 — MVP that runs end to end**
- Persistent Ed25519 identity (`identity.key` in `--data-dir`); wallet address
  derived from the peer id exactly as the legacy code did.
- TimeCoin DAG ported from `legacy/timecoin/` with correctness fixes:
  deterministic canonical hashing (the legacy hash covered `{:?}` of a
  `HashSet`, i.e. random iteration order), Ed25519 signatures with the node
  key (legacy generated a throwaway RSA key per HTTP request), balances
  derived by folding the DAG (legacy parsed wallet-address strings as
  integers, so every balance read 0), shared deterministic genesis pair.
- New: DAG replication — this did not exist in the legacy code. Gossipsub
  broadcast of new transactions + anti-entropy sync on every new connection +
  orphan pool with missing-parent re-request. JSONL persistence per node.
- HTTP API (`/status /peers /tips /transactions /balances /balance /reward
  /send /connect`) and CLI subcommands (`run status reward send balances`).
- 7 unit tests covering hashing determinism, signature enforcement, orphan
  resolution, out-of-order convergence, and persistence.

**Phase 3 — multi-node reproducibility**
- `docker-compose.yml`: one-command 3-node cluster (`docker compose up`);
  verified converging DAG and identical balances across containers.
- Relay fallback made to actually work and proven by `scripts/relay-demo.sh`
  (with mDNS disabled, so the circuit is the only path): reservation is
  requested only after the relay connection is up (a concurrent dial aborted
  it), and relays announce their address via the new `--external-address`
  flag (without a confirmed external address, reservations carry no
  addresses — `NoAddressesInReservation`). Added `--no-mdns`.
- README section for joining across real machines, with honest NAT notes.

**Verified demo** (3 local nodes + 1 late joiner): contribution rewarded on
node 2, transfer to node 3, all nodes converged to the same 4-transaction DAG,
1 tip, identical balances; node restart kept identity, DAG, and balance.
