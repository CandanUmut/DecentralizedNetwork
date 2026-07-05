# ARCHITECTURE — how this system actually works, under the hood

This is the technical companion to [VISION.md](../VISION.md). It explains every layer of
the running system — identity, the DAG ledger, the libp2p networking, discovery without
intermediaries, the security model — and the design of the layers still to come (shared
storage contracts and computation). Everything in sections 1–9 describes **code that
exists and runs today** (file references point into `src/`); section 10 is explicitly
design-for-the-future.

Reading order matters: each section builds on the previous one.

---

## 1. The bird's-eye view

Every member runs one program: `timecoin-node`. Inside it, four layers:

```
┌───────────────────────────────────────────────────────────────────┐
│  YOU                      browser / CLI                           │
├───────────────────────────────────────────────────────────────────┤
│  INTERFACE      HTTP API + web app (src/api.rs, dashboard.html)   │
│                 status · thank · send · vouch · chat · store      │
├───────────────────────────────────────────────────────────────────┤
│  LEDGER         the DAG + deterministic fold (src/dag.rs)         │
│                 transactions · validation · balances · trust      │
├───────────────────────────────────────────────────────────────────┤
│  NETWORK        libp2p swarm (src/network.rs)                     │
│                 gossip · sync · DHT · messaging · blobs · NAT     │
├───────────────────────────────────────────────────────────────────┤
│  TRANSPORT      TCP + QUIC, Noise encryption, yamux multiplexing  │
│                 (upstream libp2p crates, nothing custom)          │
└───────────────────────────────────────────────────────────────────┘
```

The critical design decision: **the ledger layer knows nothing about the network, and
the network layer holds no opinion about the ledger.** `dag.rs` is a pure state machine
— give it transactions, it gives back balances, identically on every machine. The
network's only job is to make sure every node eventually holds the same set of
transactions. This separation is why we can test consensus rules as ordinary unit tests
(21 of them) without ever opening a socket.

One more structural rule (`network.rs`): exactly **one async task owns the libp2p
swarm**. Everything else (HTTP handlers, the CLI) talks to it through a command channel
(`Command` enum) and gets answers via one-shot reply channels. No shared locks around
network state, no deadlocks — the original 2024 code wrapped the swarm in a mutex and
that pattern is precisely what strangled it.

---

## 2. Identity: a key is a person (to the network)

There are no accounts, no signups, no server that knows who you are. On first start
(`src/identity.rs`):

1. The node generates an **Ed25519 keypair** — a private key (32 bytes, stays on your
   disk in `<data-dir>/identity.key`, mode 0600) and a public key.
2. The public key is hashed into a **PeerId** — the `12D3KooW…` string. This is your
   network address-independent name: whoever can *sign with the private key* IS this
   peer, no matter which IP they appear from.
3. The PeerId is hashed again (SHA-256, hex, `0x…` prefix) into your **wallet address**
   (`dag.rs::wallet_address`).

```
 private key (secret, on your disk)
      │  derives
      ▼
 public key ──hash──► PeerId (12D3KooW…)  = your network identity
                          │  sha256
                          ▼
                      wallet (0x3fe9…)    = your economic identity
```

Consequences worth internalizing:

- **Possession of the key file is the whole identity.** Copy `identity.key` to a new
  machine and you *are* that member there. Lose it and that wallet's coins are frozen
  forever — nobody can reset a password that doesn't exist.
- Every transaction carries the signer's public key and a signature, so any node can
  verify authorship **offline**, with no authority to ask.
- Display names ("umut") are decoration written on the ledger — the wallet is the
  identity; two people can claim the same name and the system stays sound.

---

## 3. The DAG: what it is and why not a blockchain

### 3.1 The shape

A **DAG** (Directed Acyclic Graph) is a set of records where each record points at one
or more earlier records ("parents"), and no loops are possible. A blockchain is the
special case where every record has exactly one parent — a single file line. We use the
general form, sometimes called a *tangle*:

```
                       ┌──────────┐
   genesis-1 ◄─────────┤ reward A │◄──────┐
      ▲                └──────────┘       │
      │                                ┌──┴───────┐        ┌──────────┐
   genesis-2 ◄─────┐                   │ vouch B  │◄───────┤ transfer │  ← current "tip"
                   │                   └──────────┘   ┌────┤    D     │
                ┌──┴───────┐                          │    └──────────┘
                │ reward C │◄─────────────────────────┘
                └──────────┘
   (arrows point at parents; new transactions attach to the current tips)
```

- **Tips** are transactions nobody references yet — the growing edge. A new transaction
  takes the current tips as its parents (`Dag::tips()`), so the graph continually
  braids itself together.
- **Two genesis transactions** are computed identically by every node of a community
  from the community's parameters (name + daily allowance), so all members share the
  same roots — and a different community is mathematically a different graph.

### 3.2 Why a DAG instead of a chain

A blockchain needs to answer "who writes the next block?" — which forces miners,
validators, or leaders, plus fees and block times. A DAG needs no such role: **anyone
appends anytime**, and two members thanking each other in the same second simply create
two tips that the next transaction merges. There is no writer election because there is
no single write position. The price of this freedom is that the DAG gives you no global
*order* of events — and section 4 explains how we compute balances without needing one.

### 3.3 Anatomy of a transaction

Every record on the ledger is one `Transaction` (`dag.rs`), one of five kinds —
`reward` (thanks; mints), `transfer` (pays), `vouch` / `revoke` (trust statements),
`profile` (display name):

```json
{
  "id":        "95cb96…",          // sha256 of the canonical bytes below
  "kind":      "reward",
  "sender":    "0x6d67…",          // the signer's wallet (attester/payer)
  "receiver":  "0x700a…",          // who is credited / paid / vouched-for
  "amount":    20,
  "seq":       0,                  // per-sender counter, transfers only
  "memo":      "fixed my bike",
  "parents":   ["5f43…", "c1e0…"], // the tips it attached to (sorted!)
  "timestamp": 1783062003,
  "public_key":"08011220…",        // signer's key, so anyone can verify
  "signature": "aa31f0…"
}
```

Integrity comes from **canonical bytes**: every field is serialized in a fixed order
with length prefixes (`SigningFields::bytes`), then `id = sha256(bytes)` and
`signature = ed25519_sign(bytes)`. Change any bit — amount, receiver, a parent — and
the id no longer matches and the signature no longer verifies. Parents are *sorted*
before hashing; the original 2024 code hashed a `HashSet`'s random iteration order,
which meant a transaction could fail its own hash check on another machine. Determinism
bugs like that are the deadliest class in this system.

**Structural validation** (`Transaction::validate`) is the same pure function on every
node: id matches, signature verifies, and kind-specific rules — a transfer must be
signed by the *sending* wallet's key; a reward must be signed by someone *other than*
the receiver (you cannot thank yourself into money); vouches can't target yourself.

Three **contextual** rules run at insert time (`Dag::insert`):

- *Parents must exist.* If not, the transaction is parked in the **orphan pool**
  (capped at 10,000) and the network layer asks peers for the missing parents.
- *Time flows forward:* a transaction may not claim a timestamp earlier than any of its
  parents. This single rule makes wholesale backdating structurally impossible.
- *No time travel:* a transaction dated more than 5 minutes into the future is parked
  in a separate pool and only accepted once its claimed moment actually arrives
  (`Dag::release_due`, retried every 30 s). You cannot pre-farm tomorrow's allowance.

**Persistence** is an append-only file, `<data-dir>/dag.jsonl` — one JSON line per
accepted transaction. On restart the node replays and *re-validates* every line, so a
corrupted or tampered file heals to the valid subset.

---

## 4. The ledger fold: balances without a referee

This is the intellectual core of the system (`Dag::ledger_view`), so take it slowly.

**The problem:** the DAG gives every node the same *set* of transactions but no agreed
*order*. Money usually needs order ("did the spend come before the deposit?").
Blockchains buy order with mining/staking. We refuse to pay that price.

**The solution:** define balances as a **deterministic pure function of the set** — an
algorithm that, given the same bag of transactions in *any* arrival order, produces the
same balances. Then order becomes irrelevant. If two nodes hold the same transactions,
they *provably* agree. This is the property every rule below is engineered to preserve,
and what several unit tests (`two_dags_converge_regardless_of_arrival_order`,
`double_spend_resolves_identically…`) lock in forever.

The fold runs in two phases:

### Phase 1 — rewards (money creation)

All `reward` transactions are sorted by `(timestamp, id)` — a total order every node
computes identically — then counted one by one against these filters:

```
 for each reward, in (timestamp, id) order:
   1. genesis?                              → count, next
   2. [trusted view only] attester inside my vouch-neighborhood?   else skip
   3. [trusted view only] dated after attester's first trusted vouch? else skip
   4. attester's total for THIS DAY + amount ≤ daily allowance?    else skip
   5. [optional] attester's lifetime total ≤ mint_cap?             else skip
   → credit receiver
```

Rule 4 is the economy: a community with `daily_allowance = 100` simply cannot have more
than `100 × members × days` coins in existence, no matter what any client does — because
every node's fold refuses to count the excess. Note the enforcement style: an
over-allowance reward isn't rejected from the DAG (that would require order again); it
is accepted, replicated, **and worth zero everywhere**. Cheating is not forbidden; it is
made pointless, visibly.

### Phase 2 — transfers (money movement)

Every transfer carries a per-sender sequence number (0, 1, 2, …) like a check number.
The fold then runs a **fixpoint loop**:

```
 winners: for each (sender, seq), the transfer with the LOWEST id wins
          (a double-spend's loser is dead on every node, identically)
 repeat until nothing changes:
   for each winner, in (sender, seq, id) order:
     if seq is the sender's next unused number
     and sender's balance ≥ amount        → apply it
```

Walk through the attacks this quietly kills:

- **Double-spend:** Alice signs two different transfers with `seq 0` — one to X, one to
  Y. Both are valid signatures; both spread through the network. Every node picks the
  same winner (lowest hash) and the loser is permanently void. X or Y gets paid —
  the *same one* everywhere — never both.
- **Overdraft:** a modified client signs a transfer of 1000 from a wallet holding 10.
  Structurally fine, accepted into the DAG — and never applied by any node's fold,
  because the funding check fails deterministically. Network-wide balances **cannot go
  negative** (unit test: `overdraft_never_applies_network_wide`).
- **Order games:** Bob's payment to Carol is funded by Alice's payment to Bob, which
  arrives *later*. No problem — the fixpoint loop just applies it on a later pass.
  Arrival order can delay convergence by milliseconds; it can never change the result.

### The trusted view (the web of trust, mechanically)

`vouch` and `revoke` transactions build a directed graph: *I know this wallet.* For
each (voucher, vouchee) pair, the latest statement wins — trust is revocable. Your
**trust neighborhood** is a breadth-first walk from your own wallet along vouch edges,
depth-limited (default 3: you → people you vouched → people they vouched → one more).

The *trusted* fold is the same algorithm with filters 2–3 switched on: only rewards
minted by wallets in your neighborhood count, and a member's rewards only count from
the moment someone trusted first vouched for them (so nobody joins with a pre-fabricated
history of generosity). This is per-viewer — your view and mine can differ if our vouch
neighborhoods differ — but each view is still deterministic, and services use *their
own* view when accepting payment (section 8). The effect, verified live: a stranger
minted 5000 to a wallet; the raw view showed 5100, the community member's trusted view
showed exactly the 100 that a vouched member had attested.

---

## 5. libp2p: the networking, layer by layer

[libp2p](https://libp2p.io) is the modular peer-to-peer stack extracted from IPFS, used
by Ethereum's consensus layer, Polkadot, and Filecoin. We use the upstream Rust
implementation (v0.56) with zero forked code. What it actually does for us, bottom-up:

### 5.1 Addresses: multiaddrs

Every location is a self-describing **multiaddr** — a path of protocols:

```
/ip4/203.0.113.7/tcp/9000/p2p/12D3KooWEAmS…
 └── IPv4 addr ──┘└─port─┘└── expected peer identity ──┘

/ip4/1.2.3.4/tcp/9000/p2p/RELAY_ID/p2p-circuit/p2p/TARGET_ID
 └────── reach the relay ─────────┘└─ then ask it to connect me to ──┘
```

The trailing `/p2p/<PeerId>` is crucial: dialing is *addressed to an identity*, and the
handshake below cryptographically verifies the machine that answered actually holds
that identity's key. An IP can lie; the handshake cannot.

### 5.2 One connection, dissected

When node A dials node B (`SwarmBuilder` config in `network.rs::Network::new`):

```
 A                                                      B
 │  1. TCP (or QUIC) connect                            │
 │─────────────────────────────────────────────────────►│
 │  2. Noise XX handshake (TCP path):                   │
 │     ephemeral Diffie-Hellman key exchange,           │
 │     both sides prove their Ed25519 identity keys,    │
 │     session keys derived → everything after is       │
 │     encrypted + authenticated (QUIC: TLS 1.3 does    │
 │     the equivalent natively)                         │
 │◄────────────────────────────────────────────────────►│
 │  3. yamux: one encrypted connection is split into    │
 │     many independent "streams" (like lanes)          │
 │  4. per stream, multistream-select negotiates WHICH  │
 │     protocol the lane speaks:                        │
 │       lane 1: /timecoin/main/<fp>/sync/2.0.0         │
 │       lane 2: /meshsub/1.1.0        (gossipsub)      │
 │       lane 3: /timecoin/main/<fp>/msg/1.0.0          │
 │       lane 4: /ipfs/ping/1.0.0                       │
```

Step 2 is why "secure" isn't a marketing word here: after Noise, a passive observer on
the wifi sees only ciphertext, an active man-in-the-middle fails the handshake (they
can't sign as B), and A *knows* it is talking to the owner of B's key. This is the same
Noise framework Signal's protocol family comes from, from the maintained upstream
crate — we wrote zero cryptography.

### 5.3 The behaviour stack

Above the connection, libp2p composes independent **behaviours** — protocol state
machines that share the connections. Ours (`network.rs::Behaviour`), and what each is
for:

| Behaviour | Protocol id | Job |
|---|---|---|
| **gossipsub** | `/meshsub/1.1.0`, topic `timecoin/<net>/<fp>/tx/v3` | epidemic broadcast of new transactions (5.4) |
| **request-response ×3** | `/timecoin/<net>/<fp>/{sync,msg,blob}/…` | DAG sync, direct messages, storage — CBOR-encoded request/reply over a fresh stream |
| **kad** (Kademlia) | `/timecoin/<net>/<fp>/kad/1.0.0` | the DHT: find peers by id without any directory (6.2) |
| **mdns** | (UDP multicast) | zero-config discovery on the local network |
| **identify** | `/ipfs/id/1.0.0` | peers exchange their addresses + your address *as they see it* (feeds NAT logic & the DHT) |
| **ping** | `/ipfs/ping/1.0.0` | liveness + RTT |
| **autonat** | `/libp2p/autonat/1.0.0` | "am I reachable from outside?" — peers dial you back to check |
| **relay** (server+client) | `/libp2p/circuit/relay/0.2.0/*` | every node can forward traffic for NATed members (6.3) |
| **dcutr** | `/libp2p/dcutr` | upgrade relayed connections to direct ones by hole punching (6.3) |
| **upnp** | (router protocol) | ask your home router to open the port automatically |

Note the naming: every protocol we defined is namespaced by community name **and a
fingerprint of the community's rules** (`<fp>` = first 8 hex of genesis-1's id, e.g.
`/timecoin/koop/a3f91c2e/sync/2.0.0`). Two communities' nodes — or two *versions* with
different transaction rules — that accidentally meet don't even share a protocol:
isolation by construction, not by filtering. On top of that, a peer that still manages
to send repeatedly-invalid transactions collects strikes and is disconnected after 20.

### 5.4 gossipsub in thirty seconds

Naive broadcast (send to everyone you know, they forward to everyone they know) floods
the network with duplicates. Gossipsub maintains a sparse **mesh** per topic (each node
keeps ~6 mesh partners), pushes full messages only along mesh links, and *gossips*
message-ids ("I have 95cb96…") to everyone else, who can then pull what they miss.
Every message is signed by its publisher; our message id is the SHA-256 of the payload,
so identical transactions dedup network-wide automatically. Result: reliable
whole-network delivery in O(log n) hops with bounded per-node bandwidth.

---

## 6. Finding each other without an intermediary

This is the question people ask first: *if there's no server, whom do you connect to?*
Four mechanisms, layered from easiest to hardest situation:

### 6.1 Same wifi: mDNS

Nodes shout onto the local multicast address ("anyone speaking timecoin here?") and
answer each other. Two laptops on one router find each other in ~1 second with zero
configuration. This is the same mechanism your printer uses.

### 6.2 Across the internet: one address + the DHT

Somebody has to know **one** member's address — that's the join file / QR invite (it
contains bootstrap multiaddrs). From that single contact, the **Kademlia DHT** takes
over:

Every peer has an id; Kademlia defines a *distance* between ids (XOR of the bits) and
each node maintains buckets of peers at each distance scale. To find peer X, ask the
peers you know that are closest to X; they answer with even closer ones; repeat.
Lookups take O(log n) hops with each node storing only O(log n) contacts.

We run every node in DHT *server* mode, feed it addresses learned from identify and
mDNS, and refresh on every new connection. The practical effect (verified live): node
A bootstraps to B only, node C bootstraps to B only — and **A can message C by peer id
without ever having been told C's address**. The routing table found it. New members
therefore need exactly one working address, once, ever.

### 6.3 The hard case: NAT, relays, and hole punching

Most home machines sit behind NAT: they can dial out but nobody can dial in. The
escalation ladder, all from upstream libp2p:

```
 1. UPnP        node asks the router to forward the port     (often works at home)
 2. AutoNAT     peers dial you back → you learn whether      (diagnosis)
                you're publicly reachable
 3. Relay       a reachable member (any VPS, one member's    (always works)
                office machine) grants you a "reservation":
                you become dialable at
                /…relay…/p2p-circuit/p2p/YOU
                and it forwards traffic to you
 4. DCUtR       once two NATed peers are talking via the     (upgrade to direct)
                relay, they exchange predicted addresses,
                dial each other SIMULTANEOUSLY, and the
                synchronized outbound packets punch
                matching holes in both NATs
```

The relay flow in detail (`network.rs::maybe_reserve_relay`, exercised end-to-end by
`scripts/relay-demo.sh` with mDNS disabled so the circuit is the *only* path):

```
 NATed node                      Relay                      Friend
     │── connect (outbound, ok) ──►│                           │
     │── reserve a slot ──────────►│                           │
     │◄─ accepted; your circuit    │                           │
     │   address is R/p2p-circuit/YOU                          │
     │                             │◄── dial R/…circuit/YOU ───│
     │◄═ relay forwards the friend's (still end-to-end         │
     │   Noise-encrypted!) connection ═══════════════════════► │
```

Two honest notes. The relay forwards *ciphertext* — it cannot read what it carries,
because Noise runs end-to-end between the NATed node and the friend. And: there are no
default public relays for this network; a community behind full NAT needs one member
to run a reachable node. That's a deliberate non-dependency, not an oversight.

### 6.4 Staying connected

A node that finds itself with zero peers redials its bootstrap list every 30 seconds
(verified live: kill a peer, restart it, the other side reconnects unaided). Idle
connections are kept for 120 s; the DHT re-bootstraps every 5 minutes.

---

## 7. Replication: how every node ends up with the same DAG

Two complementary mechanisms (both in `network.rs`):

**Push — gossip (hot path).** You thank someone → your node inserts the transaction
locally and publishes it on the gossipsub topic → mesh partners validate and re-forward
→ ~everyone has it in O(log n) hops, typically well under a second.

**Pull — anti-entropy sync (repair path).** Gossip only helps nodes that were online.
On *every new connection*, both sides exchange DAG **tips**:

```
 A ──── SyncRequest::Tips ────────────────────────────► B
 A ◄─── SyncResponse::Txs(B's tips + ancestry, ≤512) ── B
        A inserts what it can; anything whose parents
        are missing goes to the orphan pool, and…
 A ──── SyncRequest::Get([missing parent ids]) ───────► B
 A ◄─── SyncResponse::Txs(those + THEIR ancestry ≤512)─ B
        …repeat until no orphans remain
```

Because each round returns whole *generations* (batched ancestry, 512 per response), a
returning node catches up in a handful of round trips, and a brand-new member pulls the
entire history the same way. The orphan pool is the engine: every parked transaction
*names* exactly what to ask for next. Between gossip's speed and sync's thoroughness,
the invariant that matters — *same set everywhere, eventually* — holds through sleep,
restarts, and partitions; and section 4 turns "same set" into "same balances".

---

## 8. Services and payments on top

The pattern every service follows (and future ones will copy): **a request/response
protocol + a payment convention on the ledger.**

### 8.1 Direct messages (`/timecoin/<net>/msg/1.0.0`)

A text message is a request on a fresh yamux stream to the recipient's node; `Ack` is
the delivery receipt. There is no message server — if the transport can reach the peer
(directly, via DHT-found address, or through a relay circuit), the message arrives,
end-to-end encrypted by the connection itself, sender authenticated by the handshake.
Free, because delivery is its own proof. Current limit, stated honestly: no
store-and-forward — an offline recipient means "try later" (roadmap).

### 8.2 Paid storage (`/timecoin/<net>/blob/1.0.0`)

The complete economic loop, orchestrated by `api.rs::store`:

```
 Client                                        Provider
   │── Quote{size} ────────────────────────────► │  refuses at THIS step if the
   │◄─ Price{p}  (or Refused) ───────────────────│  client is outside its trust
   │                                             │  neighborhood (no payment burned)
   │   creates transfer of p TC to provider's    │
   │   wallet, inserts locally, gossips it       │
   │── Put{data, payment_tx_id} ────────────────►│
   │                                             │  verifies IN ITS OWN LEDGER VIEW:
   │                                             │  · transfer exists, pays me ≥ p
   │                                             │  · APPLIED by the trusted fold
   │                                             │    (a double-spend loser or coin
   │                                             │    minted outside my trust web
   │                                             │    buys nothing)
   │                                             │  · never redeemed before
   │                                             │    (redemptions persisted on disk)
   │◄─ Stored{hash} ─────────────────────────────│  writes blob to blobs/<sha256>
   │   remembers 4 random slice-hashes (probes)  │
   ⋮                              …days later…   ⋮
   │── Range{hash, offset, len} ────────────────►│
   │◄─ RangeData(bytes) ─────────────────────────│  client compares against its
   │                                             │  remembered probe hash
```

Details that carry the security: content addressing means a wrong blob *is* a wrong
hash — cheating on fetch is self-evident. The custody **probes** work because the
client remembers hashes of a few random byte-ranges chosen at store time; a provider
that deleted the data (or kept only its hash) cannot answer a range it never expected —
verified live in both directions. What probes do *not* yet provide is consequence
beyond reputation: no escrow, no automatic refund (section 10).

The trust gate is where sections 4 and 8 lock together: a provider run with
`--blob-trust-depth 2` prices and verifies **against its own trusted fold** — so the
entire economy of a careful community is closed to coin minted outside its web of
trust, end to end.

---

## 9. The security model, honestly

Security here is three distinct guarantees, from three distinct mechanisms. Knowing
which one protects what is the whole game.

**Layer 1 — cryptography guarantees (strong):**

| Property | Mechanism |
|---|---|
| Nobody can forge a transaction from your wallet | Ed25519 signature over canonical bytes |
| Nobody can alter a transaction in flight or at rest | id = SHA-256 of content; re-checked on every insert & every restart |
| Nobody can read messages / sync traffic on the wire | Noise (TCP) / TLS 1.3 (QUIC), end-to-end — relays forward ciphertext |
| Nobody can impersonate a peer to your node | handshake proves possession of the PeerId's key |

**Layer 2 — determinism guarantees (strong, the novel part):**

| Property | Mechanism |
|---|---|
| All honest nodes agree on every balance | fold is a pure function of the transaction set (§4) |
| No double-spend | per-sender seq + lowest-id winner, identical everywhere |
| No overdraft, even from modified clients | unfunded transfers never apply, anywhere |
| No unlimited minting | per-attester daily allowance in the fold; excess counts as zero |
| No pre-farming the future / rewriting the past | future-parking + parent-time monotonicity |

**Layer 3 — social guarantees (real, but bounded — read carefully):**

| Threat | Defense | Residual risk |
|---|---|---|
| Sybil swarm (1000 fake keys minting) | trusted views: unvouched coin counts for zero; trust-gated services won't take it | raw view still shows it; a community that vouches carelessly imports it |
| A vouched member goes rogue | daily allowance bounds rate; `mint_cap` bounds their total in your view; revocation cuts them (and their subtree) out | damage until noticed is real, bounded, and attributable on the ledger |
| Back-filling one's own *unused past* allowance days by attaching to old parents | parent-time rule bounds it; membership anchor bounds it in trusted views; it's loudly visible on-chain | open until checkpoints (§10.3) |
| Storage provider deletes paid data | custody probes detect it | detection ≠ refund until escrow (§10.1) |
| Denial of service (garbage flood) | signature checks, orphan/future pools capped, blob size caps, inbox caps | no rate-limiting per peer yet; a determined flooder can waste CPU |

And the meta-caveats that belong in every honest document: **this code is not audited**;
the HTTP API must stay on localhost (anyone who can reach it can spend — the invite
listener is the only surface designed to be exposed, and it is read-only); metadata
(who talks to whom, when) is visible to your direct peers even though content never is;
and everything ultimately assumes a community that mostly wants the system to work.
This is armor against dishonesty and accident, not against a nation-state.

---

## 10. The future layers: storage contracts and shared computation

Everything above is running code. This section is design — the honest sketch of how
the remaining vision maps onto primitives that already exist.

### 10.1 Storage contracts (custody with consequences)

Today a cheating provider loses reputation. The upgrade is **escrow + challenges**:

```
 contract: "store blob H (size S) for T days, for X TC"
 1. client creates the payment transfer but gives the provider a
    SIGNED-BUT-UNPUBLISHED release (the ledger already supports this:
    a transaction is valid whenever it is finally inserted)
 2. periodically, client (or any delegated member) sends Range
    challenges at random offsets — exactly today's probes
 3. provider answers correctly through day T → client publishes the
    release; provider is paid
 4. provider fails a challenge → the release is never published;
    a signed "failed custody" attestation goes on the ledger for
    everyone's reputation math
```

No new consensus machinery — just withheld signatures and the existing probe protocol
with a schedule. Multi-provider replication is the same contract issued k times.

### 10.2 Shared computation (the hard one, staged by verifiability)

The brutal truth first: *arbitrary* outsourced computation is unverifiable without
either re-doing it or heavy cryptography (ZK proofs — research-grade, not for us yet).
So the design ladder climbs by how cheaply the result can be checked:

```
 rung 1: DETERMINISTIC WASM JOBS, REDUNDANT        (near-term, buildable now)
   job = wasm module + inputs, published with a price
   N providers execute; results must agree bit-for-bit
   (wasm sandboxing gives providers safety FROM the job;
   determinism gives the requester agreement ON the job)
   payment splits among agreeing majority — a lone cheater
   forfeits; colluding majority is a trust-web problem, and
   you pick providers FROM your vouch neighborhood

 rung 2: SAMPLED RE-EXECUTION                       (cheaper, needs rung 1)
   provider returns result + execution trace commitments;
   requester re-runs a random 1% of chunks; any mismatch
   voids payment (same philosophy as storage probes:
   spot-checks + consequences)

 rung 3: NON-DETERMINISTIC WORK (AI inference etc.) (needs reputation history)
   can't be bit-checked → paid on reputation + occasional
   blind duplicate jobs whose answers you already know
```

The payment convention for all rungs is the storage contract's: escrowed release
signatures, published on satisfaction. Note what is *not* needed: no new ledger rules,
no tokens, no consensus changes — jobs are just another request/response protocol
namespaced under `/timecoin/<net>/compute/…`, and the coin already flows.

### 10.3 Checkpoints (closing the last time hole, enabling forgetting)

Periodically (say weekly), members co-sign a **checkpoint**: "the DAG up to tip-set
{…} is final; its balances are B." Once a checkpoint has signatures from a quorum of a
community's trust web: transactions claiming dates before it are rejected outright
(closes the back-fill edge in §9), and nodes may prune pre-checkpoint history, keeping
only B — which caps disk growth forever. This is the one place a *little* explicit
agreement enters the design, and it's social agreement (signatures from people), not
mining.

### 10.4 Reaching everyone

Browser nodes (libp2p speaks WebTransport/WebRTC — the dashboard could become a full
peer), phone wrappers around the existing node + web UI, and bridging members who
belong to two communities' trust webs, carrying mutual aid between them. None of it
requires touching the ledger core — which is the final thing worth understanding about
this architecture: **the DAG + fold is the constitution; everything else is an app.**

---

## Appendix A — one thank, end to end

Every layer of this document in a single trace. Ayşe taps "Thank Mehmet, 20 TC, *fixed
the fence*":

```
 1. UI      POST /reward {to:"Mehmet's wallet", amount:20, memo}
 2. api.rs  checks HER allowance meter (fast-fail with remaining budget)
 3. dag.rs  takes current tips as parents, builds canonical bytes,
            sha256 → id, signs with her Ed25519 key, validates, inserts,
            appends to dag.jsonl
 4. network gossipsub publishes the JSON on timecoin/<net>/tx/v2
 5. wire    each mesh link: yamux stream inside a Noise-encrypted
            TCP/QUIC connection
 6. peers   validate independently (signature, self-mint rule, parent
            times), insert, re-forward; offline members pull it later
            via Tips-sync when they reconnect
 7. fold    every node, when next asked for balances: reward is inside
            Ayşe's daily allowance, she's inside the viewer's trust web
            → Mehmet +20. Same answer on every machine in the community.
 8. Mehmet's dashboard polls /balances: "ayşe thanked mehmet · 20 TC ·
            fixed the fence".   No server was involved at any step.
```

## Appendix B — glossary

| Term | Meaning here |
|---|---|
| **DAG / tangle** | append-anywhere graph of transactions, each pointing at earlier ones |
| **tip** | a transaction nothing references yet; where new ones attach |
| **fold** | the deterministic algorithm turning the transaction *set* into balances |
| **orphan** | valid transaction waiting for its parents to arrive |
| **PeerId / wallet** | hash of your public key / hash of your PeerId |
| **multiaddr** | self-describing address path (`/ip4/…/tcp/…/p2p/…`) |
| **Noise / yamux** | encryption handshake / stream multiplexing on one connection |
| **gossipsub** | mesh-based broadcast protocol (push data, gossip ids) |
| **Kademlia (DHT)** | distributed peer lookup by XOR distance, no directory server |
| **AutoNAT / relay / DCUtR** | reachability check / traffic forwarding by a member / hole punching |
| **vouch / trust neighborhood** | on-ledger "I know them" / BFS over vouches from your wallet |
| **daily allowance** | per-member cap on coin created by their thanks per day, part of genesis |
| **anti-entropy** | sync-on-connect repair: exchange tips, pull missing ancestry |
| **custody probe** | remembered hash of a random blob slice, checkable later |
