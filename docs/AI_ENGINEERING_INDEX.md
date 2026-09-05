# Gosslan AI Engineering Index

## Required before coding

1. `AI_RULES.md`
2. `docs/acceptance/0.12-stable-lan-chat.md` — current goal and acceptance bar
3. `AI_PROJECT_HANDOFF.md`
4. `docs/protocol-invariants.md` — when touching protocol / network / crypto / DB
5. Relevant ADR
6. Relevant tests
7. `CHANGELOG.md` history when touching a previously-fixed area

## Templates

- `docs/templates/BUG_FIX.md`
- `docs/templates/ADR.md`

## ADRs

- `0007-protocol-versioning.md`
- `0008-state-machine-boundaries.md`
- `0009-rust-typescript-contract.md`
- `0010-failure-injection-testing.md`

> Earlier ADRs `0001`–`0006` (message idempotency, outbox+ACK, E2EE, transport, no-Web-Worker,
> device fingerprint) were removed; their normative content now lives in
> `docs/protocol-invariants.md` (INV-P01…P18) and `AI_RULES.md` (INV-001…008).
> Do not re-create them as a second source of truth.

## Rule

If a document conflicts with executable code or tests, do not silently choose one. Report the conflict and determine whether the documentation or implementation is stale.
