# ASSESSMENT — Phase 0 inventory of the DecentralizedNetwork repo

> **Historical document.** This is the inventory that guided the 2026 revival. The
> `legacy/` paths it references were removed from the working tree during the
> open-source cleanup; check out the git tag **`legacy-source`** to browse them.

Date: 2026-07-02. Assessed at commit `f1cbaff` (branch `main`).

## TL;DR

**This repository does not build and cannot build as-is: it contains no `Cargo.toml`, no
`Cargo.lock`, and no vendored dependencies.** It is a snapshot of Rust *source files only*,
uploaded through the GitHub web UI ("Add files via upload"). The forked libp2p the author
remembers modifying is **not in this repository** — evidence below shows the project was
developed *inside* a local clone of `rust-libp2p` on a Windows machine, and that local
environment (fork, manifest, lockfile) was never pushed.

Consequently, "replace the fork with upstream" cannot be a diff-driven migration. The only
honest path is: keep the design and the salvageable logic, and rebuild it as a normal Cargo
project pinned to current upstream `libp2p` (0.56.x). That is what Phase 1+ does.

---

## 1. Language, build system, entry points

| Item | Finding |
|---|---|
| Language | Rust (2021-era idioms), async via tokio |
| Package manager | Cargo — **but no `Cargo.toml`/`Cargo.lock` exists in the repo** |
| Build system | None recoverable. `build.rs` exists and links `Packet.lib` from `C:\Program Files\Npcap\` — Windows-only, and nothing in the code uses packet capture (dead) |
| Entry point | `main.rs` → `#[tokio::main] async fn main()` wrapping one 1,000-line inner function `new_function_doing_everything()` |
| Intended start | `cargo run` from inside the author's local libp2p clone; node starts a libp2p swarm, an embedded rust-ipfs node, and an Axum HTTP API on `0.0.0.0:3000` |
| Secondary entry | `lib.rs` — a 12-line stub (`println!("Initializing Nova Network...")`); aspirational, not wired to anything |

## 2. Architecture map (as written)

```
main.rs (1,616 lines — everything wired here)
├── libp2p swarm:  Kademlia (IPFS DHT proto) + mDNS + request_response
│                  ("/messaging/1.0.0") + relay (server) + AutoNAT + ping
│                  Transports: TCP+DNS, WebSocket, QUIC — all Noise/TLS + Yamux
├── myipfs.rs      Embedded rust-ipfs node (add/pin/fetch files, folders, zips,
│                  metadata side-cars, provider discovery)         ~900 lines
├── webrtc.rs      webrtc-rs peer connections (see §5 — non-functional)
├── NAT stack:     stun_client (Google STUN), TURN client
│                  (hardcoded metered.ca credentials — leaked secrets!),
│                  UPnP port mapping (igd)
├── messaging.rs   request_response codec + mpsc plumbing for received messages
├── routes.rs      Axum HTTP API: /add_file /fetch_content /peers /send_message
│                  /timecoin/send /timecoin/reward /timecoin/get_balance …
└── timecoin/
    ├── peer_id.rs       IPFSPeerId = PeerId + balance field
    ├── peer_manager.rs  PeerManager: peer↔wallet map, transaction DAG
    │                    (HashMap<hash, Transaction>), tx queue, genesis bootstrap
    └── utils.rs         SHA-256 tx hashing, RSA PKCS#1v1.5 signing,
                         wallet address derivation (0x + sha256(peer_id))
network/  — a second, divergent copy of messaging/routes/myipfs/webrtc plus a
            mod.rs that re-exports functions that do not exist anywhere
            (initialize_routes, start_webrtc_connection, log_info…). Dead.
message_history.rs — lazy_static CID→timestamp log, referenced by nothing. Dead.
```

**DAG / coin design (the part worth keeping):** a transaction is
`{hash, sender, receiver, amount, parents: HashSet<hash>, signature, public_key}`.
New transactions attach to existing transactions as parents (tips), are signed, validated
(hash integrity + signature), and inserted into a `HashMap` DAG. Two genesis transactions
bootstrap the tangle. Rewards ("contribution → balance") credit a peer's wallet.

**Consensus/validation:** per-transaction validation only (hash + signature). There is no
conflict resolution, no double-spend protection beyond a local balance check, and — critically —
**no network propagation of transactions at all**. The DAG lives and dies inside one process.

**Storage:** none. Identity, DAG, and balances are all in-memory; every restart is a new
peer with an empty ledger.

## 3. The libp2p fork

### 3.1 Where it is

Not here. The evidence:

- `routes.rs:36` hardcodes
  `C:\Users\umtcn\MyProjectsNetwork\libp2p\libp2p\examples\ipfs-kad\test_dir\subdir\index.html`
  — the project lived in the **`examples/ipfs-kad` directory of a local rust-libp2p clone**.
  Building an example inside the workspace uses the workspace's (modified) crates directly,
  which is exactly how "changes to the library's own internals" would have been consumed
  without a `[patch]` section.
- `main.rs` re-exports `libp2p_swarm_derive::NetworkBehaviour` directly (a path-dependency
  style import rather than the released `libp2p::swarm::NetworkBehaviour` facade).
- `myipfs.rs` imports **rust-ipfs internals** that released versions do not expose:
  `ipfs::p2p::create_swarm`, `ipfs::p2p::transport::build_transport`, `ipfs::repo::RepoOptions`,
  `ipfs::p2p::SwarmOptions` — so the rust-ipfs dependency was also a modified local checkout.
- No `Cargo.toml` was uploaded, so the exact fork base and patch set are unrecoverable
  from this repository.

### 3.2 Which upstream version it forked from (inferred)

API fingerprints in the code date the fork base to **libp2p ~0.53–0.54 (late 2023 / mid 2024)**:
`request_response::Event` carrying `connection_id` fields, `kad::Event::OutboundQueryProgressed`
with `GetClosestPeersOk { peers: Vec<PeerInfo> }` (PeerInfo introduced in kad 0.46 / libp2p 0.54),
`SwarmConfig::with_tokio_executor`, `StreamProtocol::new("/ipfs/kad/1.0.0")`, and the
`libp2p::tcp::tokio::Transport` naming. Current upstream is **0.56.0**.

### 3.3 The modifications, inferred from the code that consumes them — tagged

Since the fork itself is gone, each customization is inferred from what the app code calls
and from the "why" the author describes (upstream didn't expose what was needed). Tags:
**(A)** now in upstream · **(B)** reproducible in our own code · **(C)** genuinely needs
patching internals · **(D)** obsolete.

| # | Customization (inferred) | Evidence | Tag | Decision |
|---|---|---|---|---|
| 1 | Custom wire protocol `/messaging/1.0.0` with raw byte codec | `messaging.rs` implements `request_response::Codec` | **(B)** — this was *always* expressible as a user-side `Codec`; upstream `request_response` (and today's `libp2p-stream`) covers it | Reimplement as our own protocol on upstream |
| 2 | Keep-alive forced on all connections | `KeepAliveHandler` in `main.rs`: a hand-rolled `ConnectionHandler` with `connection_keep_alive() = true` — writing a raw `ConnectionHandler` is the classic "had to reach into swarm internals" move | **(A)** — upstream now has `SwarmConfig::with_idle_connection_timeout`; no handler needed | Use the config knob; delete the handler |
| 3 | Direct use of `libp2p_swarm_derive` + manual event enum with `From` impls | `main.rs:64,216-271` | **(A)** — `#[derive(NetworkBehaviour)]` via the facade generates the event enum automatically | Use the facade derive |
| 4 | Hand-assembled transport stack (TCP+DNS / WS / QUIC via nested `OrTransport` + manual muxer mapping) | `main.rs:774-812` | **(A)** — `SwarmBuilder` (since 0.53, matured since) composes tcp/quic/dns/websocket with Noise+Yamux in ~5 lines | Use `SwarmBuilder` |
| 5 | Kademlia speaking the **IPFS** DHT protocol (`/ipfs/kad/1.0.0`) + 18 hardcoded public IPFS bootnodes | `main.rs:98-122,279-283` | **(B)** — protocol name is a plain config option upstream; but joining the *public IPFS DHT* to find *our* peers was always the wrong tool (none of those nodes speak `/messaging/1.0.0` or hold our DAG) | Run Kademlia under our **own** protocol id on our own bootstrap peers (deferred past MVP; see SCOPE) |
| 6 | rust-ipfs internals: build swarm/transport/repo by hand from `ipfs::p2p::*`, `ipfs::repo::*` | `myipfs.rs:19-26` | **(C→D)** — genuinely required a modified rust-ipfs; upstream rust-ipfs never stabilized this surface and the crate is effectively unmaintained today. The *feature* (content-addressed file storage) is out of MVP scope | **Drop for MVP**, documented in SCOPE.md. If revived later: `rust-ipfs`'s maintained fork (`ipfs` → `rust-ipfs`/`ipld` ecosystem) as a normal dep, or talk to an external Kubo node over HTTP |
| 7 | STUN/TURN/UPnP/WebRTC NAT stack bolted on beside libp2p | `main.rs:396-568`, `webrtc.rs` | **(A/D)** — upstream now covers this natively: AutoNAT + `libp2p-relay` (circuit v2) + `libp2p-dcutr` hole-punching, and `libp2p-upnp` exists as a behaviour. The external addresses STUN/TURN produced were fed to the swarm as fake "external addresses" that libp2p could never actually receive traffic on (a TURN allocation is useless unless the TURN client relays the bytes — nothing did) | Drop the parallel stack; use upstream relay+dcutr+upnp |
| 8 | Relay **server** behaviour compiled into every node | `main.rs:290` | **(A)** — fine upstream; kept (every node can relay for others) | Keep via upstream `relay` |

**Net result: nothing the app actually needs requires a fork of libp2p today.** Items 1–5, 7, 8
are covered by upstream 0.56 or by our own code. Item 6 (embedded IPFS) is the only true
"patched internals" case, and its feature is explicitly deferred, not silently faked.

## 4. What worked, what's stubbed, what's dead

**Plausibly worked a year ago (single machine, author's environment):**
- Swarm startup, mDNS discovery, Kademlia join of the public IPFS DHT, ping.
- `/messaging/1.0.0` request/response between two local nodes.
- IPFS add/pin/fetch through the modified rust-ipfs.
- The HTTP API (Axum 0.6 era — `axum::Server::bind`).
- Local, in-memory TimeCoin transactions between peers *registered on the same node*.

**Broken by design (would not have worked even then):**
- **DAG convergence across nodes: does not exist.** Transactions are never serialized,
  gossiped, requested, or synced. Every node has its own private ledger. This is the main
  gap between the code and the author's demo goal.
- **Balances:** `PeerManager` stores wallet *address strings* in `peer_map` and computes
  balance as `wallet.parse::<u64>()` — an address like `0x3fe9…` never parses, so every
  balance reads 0; `reward_timecoin` then *overwrites the wallet address* with a number
  string. Two incompatible balance models (this one, and the unused `IPFSPeerId.balance`
  field) coexist. `create_transaction` "deducts" balances on **clones** of map keys and
  throws the result away.
- **Transaction hashing is non-deterministic:** parents are hashed via `format!("{:?}")`
  of a `Vec` collected from a `HashSet`, whose iteration order is random per process —
  the same transaction re-validated elsewhere (or even re-collected) can fail its own
  hash check. Fatal for any cross-node validation.
- **Transaction signing keys are throwaway:** `/timecoin/send` generates a *fresh RSA-2048
  keypair per HTTP request*, so signatures prove nothing about the sender.
- **WebRTC:** creates `RTCPeerConnection`s but there is no signaling — no offer/answer/ICE
  exchange ever happens, so no connection could ever establish. It also fires
  `PeerConnectionEstablished` immediately upon *creating* the object (a fake success).
- **TURN:** allocates a relay address, feeds it to libp2p as an external address, and then
  nothing relays traffic. Also: **live TURN credentials are hardcoded in the source** —
  treated as leaked; do not reuse.
- **Identity is not persisted:** `Keypair::generate_ed25519()` on every start.

**Dead code:** the entire `network/` directory (divergent duplicates + `mod.rs` re-exporting
nonexistent symbols — `mod network` is never declared), `message_history.rs` (never imported),
`lib.rs`, `build.rs` (Npcap), `KeepAliveHandler` (never installed into the swarm), the
commented-out CLI (`Opt`/`CliArgument` parsed but unused), `network/state.rs` (does not
even name-resolve: `Swarm` without type parameter, `IPFSHandler` unimported).

**Concurrency hazard worth noting:** the swarm lives in `Arc<Mutex<Swarm>>` and the main
event loop holds the lock across `select_next_some().await` — every HTTP handler that
touches the swarm contends with the event loop; reconnect tasks then try to re-lock it.
This worked as well as it did because nothing else ran. The rebuild uses the standard
pattern instead: the swarm owned by one task, commands in via an mpsc channel.

## 5. Smallest end-to-end path that could realistically run today

There is no path that runs *today* — no manifest, absent fork, and the DAG was never
networked. The smallest **honest** path to the author's demo goal (N nodes, contributions,
converging DAG, consistent balances):

1. New `Cargo.toml` pinned to upstream `libp2p 0.56` (tcp+quic, noise, yamux, gossipsub,
   mdns, identify, ping, relay, dcutr, autonat, upnp — all released crates, no fork).
2. Persist the Ed25519 node key (`--data-dir`); peer id stable across restarts; wallet
   address derived from it exactly as before (`0x` + sha256).
3. Port the TimeCoin DAG with the bugs fixed: deterministic hashing (sorted parent list,
   canonical byte encoding), Ed25519 signatures with the *node's* key, balances computed
   *from the DAG* (rewards mint, transfers move), tips = parentless-children frontier.
4. Add the missing 20%: a gossipsub topic for transaction broadcast + a request/response
   sync protocol (our `/timecoin/sync/1.0.0` — the spiritual successor of `/messaging/1.0.0`)
   for anti-entropy when a node joins, with orphan re-request. This is what makes the DAG
   converge.
5. Keep the HTTP API surface (status/peers/tips/balance/send/reward) + a tiny CLI.
6. docker-compose for the N-node cluster; documented bootstrap + relay path for
   cross-machine.

## 6. ⚠️ Flag to the author (per your "pause and flag me" instruction)

Phase 0 revealed exactly the condition you asked to be flagged on, so read this first:

- **The fork could not be "replaced" because it was never in the repo.** What was pushed is
  application source only. De-forking therefore = rebuilding the same design as a normal
  Cargo project on upstream 0.56. Nothing here needs a libp2p fork anymore (§3.3).
- **Reaching your demo goal required writing the DAG sync layer fresh**, because transaction
  propagation between nodes did not exist in the old code (§4). That is new code, not a port
  — I've kept it as small as possible and it follows your own design (tips, parents,
  signatures, reward-minting).
- **Dropped for the MVP** (all listed in SCOPE.md, none silently): embedded rust-ipfs file
  storage (the one true "patched internals" dependency), WebRTC, the STUN/TURN stack
  (including the leaked credentials), and joining the public IPFS DHT.

Your prompt said "continue to Phase 1 without waiting", and everything cut is either outside
your stated Phase 2 target list or provably non-functional, so I proceeded — but if embedded
IPFS storage (or anything above) is a must-have, say so and it goes back on the roadmap.
