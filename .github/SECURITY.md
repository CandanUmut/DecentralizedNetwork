# Security policy

This is young, **unaudited** software for communities of people who trust each other.
Do not use it to protect anything valuable from a determined adversary — the README's
"what doesn't work" section is the authoritative list of known limitations.

## Reporting a vulnerability

If you find a way to break the ledger rules (mint beyond the allowance, spend coins you
don't have, make honest nodes' balances diverge, impersonate a peer, or crash a node
with crafted input), please report it privately rather than opening a public issue:

- Use GitHub's **"Report a vulnerability"** (Security tab → private advisory), or
- email the repository owner (address on their GitHub profile).

Include the smallest reproduction you can. Determinism bugs (two honest nodes computing
different balances from the same transactions) are treated as the highest severity.

There is no bounty program — just gratitude, a fix, and credit in the CHANGELOG if you
want it.
