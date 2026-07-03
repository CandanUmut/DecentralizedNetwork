# ROADMAP — from a converging ledger to a network worth joining

The vision: a decentralized network where people share compute, memory, storage,
simulations, and safe communication — and *earn* by contributing. This document is the
staged path there, with the honest hard problems named at each stage. The guiding rule
stays the same as in the revival: **each stage must produce something real and runnable
before the next begins**, and nothing gets faked to look further along than it is.

## Design philosophy

1. **Trust before features.** Nobody stores their files on, or lends their CPU to, a
   network whose currency can be counterfeited. The ledger's integrity is the product;
   everything else is an app on top of it.
2. **Earning = attested value received.** TimeCoin is minted when someone *attests that
   someone else provided value* — a signed receipt, not self-declared work. Every mint
   is attributable to the wallet that vouched for it.
3. **Services are protocols, not platform code.** Storage, messaging, compute — each is
   its own libp2p protocol + payment convention on the shared ledger. The node stays a
   small kernel: identity, transport, ledger, discovery.
4. **Verifiable work first.** Offer services whose delivery the payer can verify cheaply
   (content-addressed storage: hash matches; messaging: you got the message). Unverifiable
   work (arbitrary compute) comes last, because it needs reputation or redundancy.

## Stage 1 — a ledger you can charge against *(building now)*

The MVP converges, but anyone can mint and a modified client can double-spend. Fixes:

- **Attested rewards:** a reward transaction is only valid if its *signer's* wallet
  differs from the recipient's — you cannot mint to yourself. The minter's wallet is
  recorded as `sender`, so every coin's origin is attributable. (This kills lazy/self
  minting; it is **not** Sybil resistance — one human with two keys can still
  self-attest. Sybil resistance arrives in Stage 3 as social trust, not proof-of-X.)
- **Sequence numbers + deterministic conflict resolution:** every transfer carries a
  per-sender sequence number. Balances are computed by a deterministic fold over the
  transaction *set*: rewards first, then transfers applied in fixpoint passes ordered by
  (sender, seq, hash); a transfer applies only if its seq is the sender's next and funds
  suffice; same-seq conflicts resolve to the lowest hash, the loser is dead. Result:
  double-spends resolve identically on every node, and **network-wide balances can never
  go negative**, even against modified clients — without adding any consensus machinery.
- **Delta sync:** on connect, peers exchange DAG *tips* and pull only missing ancestry
  (instead of full id lists).
- **Named networks:** `--network <name>` derives a distinct genesis and gossip topic, so
  a family, a koop, a research group each run their own ledger without interference.

## Stage 2 — the first services, each with a payment convention *(building now)*

- **Safe comms (free):** direct peer-to-peer text messages over the already-encrypted,
  already-authenticated libp2p streams (`/timecoin/msg`). This revives the legacy
  messaging feature, working this time. Free, because delivery is its own proof.
- **Paid storage (`/timecoin/blob`):** the seed of the storage marketplace, kept to the
  verifiable core: ask a provider for a quote (price per stored blob by size), pay via a
  normal ledger transfer, hand over the blob with the payment transaction id; the
  provider verifies the payment in its own DAG before storing; anyone with the hash can
  fetch. Content addressing makes cheating detectable by construction: a wrong blob has
  the wrong hash. *Not yet solved here (Stage 4 material): proof of continued storage
  over time, replication contracts, retrieval markets.*
- **Observability:** a tiny web dashboard on the node (peers, tips, balances, live tx
  feed) — because a network you can't see doesn't feel real.

## Stage 3 — a network of people, not processes

What's needed before strangers can coexist (design sketches, not yet built):

- **Web-of-trust minting weight.** Wallets accumulate *vouches* (a signed statement, on
  the ledger). A reward's "weight" discounts by the minter's trust distance from the
  viewer. Communities effectively define their own money supply socially — which is what
  a koop already is. This is the honest answer to Sybil: not proof-of-work, but
  proof-of-being-known.
- **Rate limits & spam control:** per-wallet mint budgets per epoch (community-config),
  gossip validation quotas per peer, and proof-of-relay for message forwarding.
- **Aliases & profiles:** human-readable names bound to wallets by on-ledger records
  (first-claim + community vouching), replacing raw 0x… addresses in every UI.
- **Bootstrap infrastructure:** a community config file (network name, bootstrap +
  relay addresses, initial trusted wallets) shareable as one small JSON/QR — "join my
  network" becomes sending one file.

## Stage 4 — the marketplace layer

- **Storage contracts:** provider commits to store blob H for T days for X TC; periodic
  random challenges (send me bytes i..j of H) prove continued custody; missed challenges
  slash escrowed payment. Escrow = 2-of-2 co-signed release transactions (the ledger
  already supports the primitive: a transfer signed but not yet published).
- **Compute jobs:** start with *deterministic, verifiable* jobs only — WASM tasks where
  the requester can re-execute a random sample, or N-of-M redundant execution with
  majority output. Non-verifiable AI inference comes only after reputation exists.
- **Shared simulations / AI:** these are *applications* of compute + storage + comms
  above; they need no new network primitives, "just" the three markets working.

## Stage 5 — reach

Browser nodes via WebTransport/WebRTC (upstream libp2p supports both), mobile wrappers,
a Kademlia DHT under our own protocol id once bootstrap lists stop scaling, IPFS interop
for public content, audits before anything is called safe.

## What I would *not* do

- No token sale, no global "mainnet", no proof-of-work/stake. TimeCoin's model —
  community-scoped, attestation-minted, socially trusted — is a different animal on
  purpose; grafting blockchain economics onto it would kill the point.
- No custom cryptography, ever. Ed25519 + Noise + SHA-256 from maintained crates.
- No feature before its trust prerequisite: paid compute before reputation would just
  fund fraud.

## Sequencing rationale

Stage 1 and 2 are being built now, in this order, because each unlocks the next: the
conflict-resolving fold (1) is what makes accepting payment (2) safe; storage-for-pay
(2) is the template every later market (4) copies; and the web-of-trust (3) reuses the
attestation machinery rewards already have. At every point the network stays runnable
end to end.
