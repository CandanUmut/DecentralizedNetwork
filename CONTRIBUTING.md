# Contributing

Thanks for looking under the hood. This project values **working over complete** and
**honest over impressive** — the docs never claim more than what has actually been run.
Contributions are expected to keep both.

## Build, test, run

```bash
cargo build                 # debug build
cargo test                  # 21 unit tests (ledger rules, trust, economy)
cargo clippy -- -D warnings # CI enforces zero warnings
./scripts/relay-demo.sh     # end-to-end: relay reservation + circuit-only sync
docker compose up --build   # 3-node cluster on localhost:3001..3003
```

Quick two-node loop while developing:

```bash
cargo run -- run --data-dir /tmp/a --port 9001 --api 127.0.0.1:3001 --invite off
cargo run -- run --data-dir /tmp/b --port 9002 --api 127.0.0.1:3002 --invite off \
    --bootstrap /ip4/127.0.0.1/tcp/9001
```

## Where things live

| Path | What |
|---|---|
| `src/dag.rs` | The heart: transactions, validation, the deterministic ledger fold (allowance, trust, double-spend rules), persistence. Most consensus-relevant code is here and unit-tested here. |
| `src/network.rs` | libp2p wiring: gossipsub, sync protocol, messaging, blob storage, DHT, relay/NAT. One task owns the swarm; commands come in via a channel. |
| `src/api.rs` | HTTP API + orchestration (store = quote→pay→put, custody probes). |
| `src/main.rs` | CLI and node startup. |
| `src/dashboard.html`, `src/invite.html` | The web UI, embedded via `include_str!`. Self-contained, no build step, no external assets. |
| `docs/ROADMAP.md` | Where this is going, and the honestly-open problems. |
| `docs/history/` | How this came to be (assessment of the original code, scope decisions, original notes). The pre-rewrite source is in git history at commit `2192a1b` (tagged `legacy-source` locally). |

## Ground rules

1. **Never fake.** No stubs that pretend to work, no claims in docs that weren't
   exercised. If something can't be done cleanly, document the limitation instead.
2. **Determinism is sacred in `dag.rs`.** Anything that feeds the ledger fold must be a
   pure function of the transaction set (plus explicitly-passed context like `now`).
   If two nodes could disagree, it's a consensus bug — write a test proving convergence.
3. **Changing validation or the fold is a network break.** Genesis/params changes split
   the network by design; call it out loudly in the PR and CHANGELOG.
4. **No new dependencies without cause.** Especially no alternative crypto — Ed25519,
   Noise, and SHA-256 from the existing maintained crates only.
5. **Small PRs, tests included, CHANGELOG updated.** CI runs `cargo test` and
   `clippy -D warnings` on every push.

## Good first contributions

See "What I'd do next" in the README and the open problems in `docs/ROADMAP.md` —
checkpoints, store-and-forward messaging, storage escrow, trust decay, translations of
the invite page and VISION.md.
