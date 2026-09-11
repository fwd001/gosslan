# Gosslan AI Engineering Index

## Required before coding

1. `AI_RULES.md`
2. `docs/acceptance/1.0-release.md` — current goal and acceptance bar
3. `AI_PROJECT_HANDOFF.md`
4. `docs/protocol-invariants.md` — when touching protocol / network / crypto / DB
5. `docs/design-guidelines.md` — when touching UI (圆角 / hover / 配色 / 窗口边界)
6. Relevant ADR
7. Relevant tests
8. `CHANGELOG.md` history when touching a previously-fixed area

## Templates

- `docs/templates/BUG_FIX.md`
- `docs/templates/ADR.md`

## ADRs

- `0007-protocol-versioning.md`
- `0008-state-machine-boundaries.md`
- `0009-rust-typescript-contract.md`
- `0010-failure-injection-testing.md`
- `0011-gossip-envelope-authentication.md`
- `0012-logical-sequence-ordering.md`
- `0013-transport-priority-queues.md`
- `0014-multi-path-connection-selection.md`

> Earlier ADRs `0001`–`0006` (message idempotency, outbox+ACK, E2EE, transport, no-Web-Worker,
> device fingerprint) were removed; their normative content now lives in
> `docs/protocol-invariants.md` (INV-P01…P20) and `AI_RULES.md` (INV-001…008).
> Do not re-create them as a second source of truth.

## Rule

If a document conflicts with executable code or tests, do not silently choose one. Report the conflict and determine whether the documentation or implementation is stale.
