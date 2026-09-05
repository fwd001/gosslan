# Gosslan AI Engineering Index

## Required before coding

1. `AI_RULES.md`
2. `AI_PROJECT_HANDOFF.md`
3. Relevant ADR
4. `docs/protocol-invariants.md`
5. Relevant tests
6. `CHANGELOG.md` history when touching a previously-fixed area

## Templates

- `docs/templates/BUG_FIX.md`
- `docs/templates/ADR.md`

## ADRs

- `0001-message-idempotency.md`
- `0002-outbox-ack-reliability.md`
- `0003-e2ee-static-x25519.md`
- `0004-transport-abstraction.md`
- `0005-no-web-worker.md`
- `0006-device-fingerprint-identity.md`
- `0007-protocol-versioning.md`
- `0008-state-machine-boundaries.md`
- `0009-rust-typescript-contract.md`
- `0010-failure-injection-testing.md`

## Rule

If a document conflicts with executable code or tests, do not silently choose one. Report the conflict and determine whether the documentation or implementation is stale.
