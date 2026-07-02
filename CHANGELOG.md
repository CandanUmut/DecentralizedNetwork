# CHANGELOG

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
