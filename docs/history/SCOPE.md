# SCOPE — what the revival MVP was, and what it was not

> **Historical note:** this documents the scope of the initial revival (v0.1.0).
> Several deferrals below have since shipped in v0.2.0 — attested minting,
> network-wide overdraft immunity, deterministic double-spend resolution, paid
> blob storage, and direct messaging — plus the web of trust and the
> daily-allowance economy. See [../ROADMAP.md](../ROADMAP.md) for the current
> state and the repo-root CHANGELOG.md for what changed. The `legacy/` source
> mentioned below lives at the git tag **`legacy-source`**.

This MVP exists to prove one thing end to end: **N nodes on current upstream libp2p
discover each other, gossip signed TimeCoin contribution records into a shared DAG, and
converge on the same ledger and balances.** Everything else is deferred, on purpose.

## IN the MVP

- **Persistent node identity** — Ed25519 keypair on disk (`--data-dir`), stable PeerId
  across restarts; wallet address derived from the peer id (`0x` + sha256, as in the
  original design).
- **Secure transport** — upstream libp2p TCP + QUIC with Noise encryption and Yamux
  multiplexing. Peer authentication is standard libp2p (connections are bound to peer ids).
- **Peer discovery** — mDNS on the local network; explicit `--bootstrap <multiaddr>` list
  for everything else; identify + ping.
- **NAT path** — AutoNAT (am I reachable?), relay client + DCUtR hole punching via a
  configurable relay, UPnP port mapping where the router allows it. Every node can also
  serve as a relay for others. Honest limitations documented in the README.
- **TimeCoin DAG** — transactions `{sender, receiver, amount, parents, timestamp, sig}`;
  deterministic canonical hashing; Ed25519 signatures with the node key; two well-known
  genesis transactions; new transactions attach to current tips; full validation
  (hash, signature, parent existence, non-negative balance for transfers).
- **Contribution → balance rules** — `reward` transactions mint TimeCoin to a wallet
  (the "contribution earned" primitive); `send` transactions move it. Balances are
  *derived from the DAG*, never stored as mutable counters.
- **DAG convergence** — gossipsub broadcast of new transactions + a request/response sync
  protocol (`/timecoin/sync/1.0.0`) that fetches the whole DAG on connect and re-requests
  missing parents (orphan resolution). DAG persisted to disk per node.
- **Observability** — HTTP API (`/status`, `/peers`, `/tips`, `/balance`, `/transactions`,
  `/send`, `/reward`) and a `status` CLI subcommand; enough to watch convergence happen.
- **One-command local cluster** — docker-compose with N nodes; cross-machine join
  documented step by step.

## DEFERRED (explicitly not in this build)

| Deferred | Why / what it would take |
|---|---|
| **IPFS / file storage** (`myipfs.rs`, ~900 lines) | Depended on a locally modified rust-ipfs whose internals aren't exposed by any released crate; the crate is effectively unmaintained. Revive later via a maintained IPFS implementation as a normal dependency, or an external Kubo node over HTTP. |
| **WebRTC transport** (`webrtc.rs`) | The old code had no signaling, so it never carried a connection. Upstream `libp2p-webrtc` is the right path if browsers become a target. |
| **STUN / TURN / manual NAT stack** | Replaced by libp2p-native AutoNAT + relay + DCUtR + UPnP. The old TURN credentials were hardcoded in source and must be considered leaked/dead. |
| **Public IPFS DHT participation** (18 bootnodes, `/ipfs/kad/1.0.0`) | Wrong network for our protocols. A Kademlia DHT under our *own* protocol id is the natural next step once the network outgrows explicit bootstrap lists. |
| **Shared compute / memory / storage marketplace, simulations, AI workloads** | The long-term vision. Nothing here pretends to do it. |
| **Social layer, PRU-DB truth engine, aliases, koop-app** (per Sprint_plan.md / Koop.md) | Application layers for after the network layer is trustworthy. |
| **Consensus beyond signature + parent validation** | No double-spend resolution across conflicting DAG branches, no finality, no Sybil resistance (any node can mint via `reward`). Fine for a cooperative demo among trusted peers; **not** money. Listed in README's "what doesn't work". |
| **Windows Npcap build step** (`build.rs`) | Nothing used it. Dropped. |

## Honesty notes

- This is **not audited** and **not safe for untrusted participants**: anyone on the
  network can submit reward transactions (minting), and there is no spam control.
- Transport encryption and peer authentication are exactly what upstream libp2p provides —
  no custom crypto was written for the network layer.
- The legacy source is preserved under `legacy/` for reference; the new node lives in
  `node/` and shares its design (and salvaged logic) rather than its bugs.
