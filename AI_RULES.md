# Gosslan AI Development Rules

> Version: 0.12
> Status: Pre-release / Stable LAN Chat
> Project type: Tauri v2 + Vue 3 + TypeScript + Rust + SQLite
> Primary goal: Stable LAN Chat
>
> This document is the primary engineering rule for AI-assisted development.
> When a task conflicts with this document, stop and resolve the conflict before coding.

---

# 1. Project Positioning

Gosslan is currently a **pre-release project**.

The immediate goal is not to build a complete distributed communication platform.

The immediate goal is:

> **Make the existing LAN chat stable, reliable, understandable, and easy to extend.**

The current priority is:

```text
Stable LAN Chat
    ↓
Correct message delivery
    ↓
Correct persistence
    ↓
Correct E2EE
    ↓
Correct offline recovery
    ↓
Good user experience
    ↓
Simple extensibility
```

Do not sacrifice current chat stability for future architecture.

---

# 2. v0.12 Scope

## 2.1 v0.12 Core Goals

The following are P0:

* LAN device discovery
* Friend request / accept
* Online / offline state
* Single chat
* Text messages
* Image messages
* File messages
* Chat history
* Unread messages
* Delivery state
* Read receipt
* Failed state
* Offline message persistence
* Automatic resend
* Message deduplication
* E2EE
* SQLite persistence
* Restart recovery
* Notification
* Basic Windows / macOS / Android stability
* Two-device real LAN communication

The following are P1:

* Shared directory
* Friend delete / re-add
* Tray behavior
* Basic stress testing
* UI stability improvements

---

# 3. Explicitly Frozen for v0.12

Do NOT proactively implement, redesign, optimize, or expand:

* Bluetooth transport
* QUIC
* mDNS
* Cross-subnet communication
* Server relay
* Account system
* Noise XX
* Advanced Mesh routing
* Large-scale relay optimization
* 500–1000 node optimization
* New transport implementations
* Large-scale architecture refactoring
* New distributed-system mechanisms
* Future roadmap features not required by the current task

Existing code related to these features may remain.

Preserve useful interfaces and extension points when practical.

But:

> **Do not implement future features merely because the architecture could support them.**

---

# 4. Engineering Philosophy

Priority order:

```text
Correctness
>
Reliability
>
Simplicity
>
Maintainability
>
Extensibility
>
Performance optimization
>
Future-proofing
```

Prefer:

```text
reuse existing code
>
small local fix
>
small abstraction
>
large refactor
```

Do not introduce complexity unless the current requirement actually needs it.

A theoretically elegant architecture is not automatically better.

For this project:

> A simple solution that is correct is better than a sophisticated solution that creates more failure paths.

---

# 5. AI Task Complexity

Not every task requires full architectural analysis.

## L1 — Trivial

Examples:

* UI text
* CSS
* layout
* icon
* simple component change
* small pure function
* obvious bug
* simple validation

Process:

```text
Locate → Modify → Test
```

Do not perform unnecessary architecture analysis.

---

## L2 — Normal Feature

Examples:

* normal chat feature
* notification behavior
* unread behavior
* UI interaction
* existing API integration
* small database query change

Process:

```text
Locate existing logic
→ Reuse existing implementation
→ Identify affected files
→ Make minimal change
→ Test
```

Do not redesign the system unless required.

---

## L3 — Core Reliability

Examples:

* protocol
* message lifecycle
* network transport
* discovery
* E2EE
* SQLite schema
* outbox
* ACK
* retry
* deduplication
* connection lifecycle

Process:

```text
Understand data flow
→ Identify invariants
→ Identify failure paths
→ Plan minimal change
→ Implement
→ Add regression test
→ Run relevant E2E tests
```

L3 tasks require strict engineering discipline.

---

## Important Rule

> Do not upgrade an L1/L2 task into an L3 architecture project.

Do not turn:

```text
"fix this button"
```

into:

```text
"let's redesign the state architecture".
```

Do not turn:

```text
"fix this message bug"
```

into:

```text
"let's rewrite the entire messaging subsystem".
```

---

# 6. Existing Code First

Before creating new logic:

1. Search the repository.
2. Find existing implementation.
3. Understand how it currently works.
4. Reuse it when possible.
5. Modify the existing path when appropriate.

Do not create a second implementation of an existing responsibility.

Examples:

Bad:

```text
Existing message send logic
+
new message send helper
+
new message service
```

Preferred:

```text
Existing message send logic
+
minimal modification
```

Only introduce a new abstraction when the existing structure genuinely prevents a correct implementation.

---

# 7. Source of Truth

Prefer the following order:

```text
Actual implementation
>
Existing tests
>
Protocol definitions
>
AI_RULES.md
>
ADR
>
Handoff documentation
>
README
>
Future roadmap
```

Documentation describes the system.

The actual code and tests determine current behavior.

If documentation and implementation disagree:

1. Do not blindly assume either one is correct.
2. Inspect the current data flow.
3. Determine intended behavior.
4. Fix documentation or implementation as appropriate.
5. Do not silently create a third behavior.

---

# 8. Core Message Invariants

These invariants are mandatory.

## INV-001 — Stable Message ID

Every logical message must have a stable `msg_id`.

Retries must reuse the same `msg_id`.

Do not create a new message ID merely because transmission failed.

---

## INV-002 — Idempotency

Receiving the same `msg_id` multiple times must not create duplicate logical messages.

This applies to:

* direct delivery
* retry
* reconnect
* outbox resend
* gossip
* duplicate packets

---

## INV-003 — Outbox Before Delivery

Reliable outgoing messages must follow:

```text
Create message
    ↓
Persist message
    ↓
Persist outbox
    ↓
Attempt network delivery
```

Do not rely solely on memory.

---

## INV-004 — ACK Means Persisted

A successful TCP write does NOT mean the message was delivered.

The sender may consider a message delivered only after the receiver confirms the appropriate persistence/processing point through the protocol ACK.

Conceptually:

```text
TCP send
≠
Delivered
```

---

## INV-005 — No Silent Message Loss

Encrypted or network messages must not disappear silently.

If processing fails:

```text
success
or
explicit failure / retry / diagnostic path
```

Never:

```text
catch error
→ ignore
→ pretend nothing happened
```

---

## INV-006 — UI State Must Reflect Reality

Do not leave a message permanently in:

```text
sending
```

when the system already knows that the operation failed.

Likewise:

```text
network temporarily unavailable
```

must not automatically mean:

```text
message permanently failed
```

---

## INV-007 — Device Identity Persists

The device identity must remain stable across application restarts.

Do not regenerate identity keys or device fingerprints on every launch.

---

## INV-008 — Persistence Is Part of Reliability

Important state must survive restart.

At minimum:

* device identity
* cryptographic identity
* friends
* messages
* message status
* outbox state where applicable

---

# 9. Message Lifecycle

The intended reliable flow is:

```text
User sends message
        ↓
Create msg_id
        ↓
Persist message
        ↓
Persist outbox
        ↓
Encrypt
        ↓
Send
        ↓
Receiver decrypts
        ↓
Receiver persists / deduplicates
        ↓
Receiver ACK
        ↓
Sender marks delivered
        ↓
Remove outbox
```

Do not bypass this flow for convenience.

---

# 10. Temporary Network Failure

Network failure is not automatically permanent message failure.

Example:

```text
A sends message
        ↓
B offline
        ↓
message remains in outbox
        ↓
B comes online
        ↓
connection restored
        ↓
message resent
        ↓
ACK received
        ↓
outbox removed
```

This behavior is a core requirement.

---

# 11. Protocol Rules

All protocol messages must use the existing protocol layer.

Do not create random protocol formats in unrelated modules.

Before modifying a protocol message, check:

1. Who sends it?
2. Who receives it?
3. Is it persisted?
4. Is it encrypted?
5. Is it idempotent?
6. Does it require ACK?
7. Does it interact with outbox?
8. Does reconnect/retry change its behavior?
9. Does it affect gossip or forwarding?
10. Does the TypeScript side depend on it?

---

# 12. Pre-Release Protocol Compatibility

## IMPORTANT

Gosslan has **not been released**.

There are currently no production users or historical clients that must remain compatible.

Therefore:

> **Do not add compatibility layers for hypothetical old versions.**

Do not preserve obsolete protocol behavior merely because an older development build might have used it.

Do not add:

* legacy protocol branches
* unnecessary version adapters
* compatibility wrappers
* duplicate old/new message formats
* migration code for versions that never shipped

unless there is a real requirement.

---

## 12.1 Breaking Protocol Changes Are Allowed

Before release, a protocol change may intentionally break the current development version.

Examples:

```text
old message format
→ new message format
```

or:

```text
old protocol field
→ removed
```

or:

```text
old state machine
→ corrected state machine
```

This is acceptable when it produces a cleaner and more correct current implementation.

However:

> Breaking does not mean careless.

For a protocol-breaking change:

1. Update sender.
2. Update receiver.
3. Update related tests.
4. Update E2E tests.
5. Update protocol documentation if necessary.
6. Remove obsolete compatibility code.
7. Verify the complete message flow.

---

# 13. Pre-Release Database Rules

The same principle applies to SQLite.

Because Gosslan has not been released:

> **Database schema cleanup and breaking changes are allowed when they simplify the current system or fix incorrect design.**

Do not build complicated migration systems solely for hypothetical unreleased versions.

---

## Allowed

Examples:

```text
bad column
→ replace column

incorrect schema
→ redesign schema

duplicate state
→ consolidate state

temporary development table
→ remove table
```

when the change is justified.

---

## Still Required

A database change must:

* keep the current application consistent
* correctly initialize a fresh database
* correctly handle the current development database
* update related queries
* update tests
* avoid silent data corruption

If an existing development database can simply be recreated, say so explicitly.

Do not create ten layers of migration code to protect a database that has never shipped.

---

# 14. Compatibility Rule

Use this decision:

```text
Has this behavior been released to real users?
        │
        ├── YES → preserve compatibility
        │
        └── NO
             ↓
       Is breaking change useful?
             │
             ├── YES → allow breaking change
             │
             └── NO → keep existing behavior
```

The purpose of compatibility is to protect real users.

Do not create compatibility complexity without a real compatibility requirement.

---

# 15. State Machines

When modifying core state, understand the existing state machine.

Important states include:

### Message

```text
sending
  ↓
delivered
  ↓
read
```

Failure paths may include:

```text
sending
  ↓
retry
  ↓
delivered
```

or:

```text
sending
  ↓
failed
```

depending on the actual error.

---

### Connection

Conceptually:

```text
disconnected
→ connecting
→ connected
→ disconnected
```

Do not invent UI-only connection states that disagree with the Rust/network layer.

---

### File Transfer

Respect the existing:

```text
offer
→ accept
→ transfer
→ complete / failed
```

flow.

Do not create an independent file-transfer lifecycle in the frontend.

---

# 16. Rust / TypeScript Boundary

Rust is responsible for:

* network
* discovery
* encryption
* database
* protocol
* reliable delivery
* file transfer
* platform integration

TypeScript/Vue is responsible for:

* UI
* interaction
* presentation state
* user-facing error display
* frontend orchestration

Do not move core network/protocol logic into the frontend simply because it is easier to implement.

Do not create a second business implementation in TypeScript.

---

# 17. Frontend State Rules

Frontend stores should reflect backend reality.

Do not let the UI invent:

* delivery state
* connection state
* encryption state
* online state
* persistence state

Example:

Bad:

```text
send() succeeded
→ immediately show delivered
```

Preferred:

```text
send()
→ backend processes message
→ ACK
→ update delivered
```

Optimistic UI is allowed only when failure/rollback behavior is clearly defined.

---

# 18. Database Rules

Before modifying the database:

1. Find existing schema.
2. Find existing query functions.
3. Find all callers.
4. Check tests.
5. Determine whether the change is actually necessary.

Do not create duplicate storage for the same concept.

Bad:

```text
existing unread state
+
new unread state
```

Preferred:

```text
existing source of truth
+
correct it
```

---

# 19. Crypto Rules

E2EE is mandatory.

Never introduce:

```text
plaintext fallback
```

when encryption fails.

Never silently:

* disable encryption
* bypass encryption
* accept unverifiable keys
* ignore authentication failures

If cryptographic processing fails:

```text
explicit error
or
explicit retry path
```

not silent fallback.

Do not replace existing cryptographic primitives without a strong reason.

---

# 20. Error Handling

Never hide core errors.

Avoid:

```rust
let _ = something();
```

when the result matters.

Avoid:

```text
catch
→ ignore
```

especially for:

* network
* database
* encryption
* protocol
* file transfer
* persistence

Errors should either:

1. be handled correctly,
2. be returned,
3. be logged with useful context,
4. or be transformed into a meaningful user-visible state.

---

# 21. Bug Fix Workflow

For a bug:

```text
Symptom
↓
Reproduce
↓
Trace actual data flow
↓
Locate break point
↓
Find root cause
↓
Minimal fix
↓
Regression test
↓
Run relevant verification
```

Do not start with:

```text
"Let's refactor the architecture."
```

unless the architecture is actually the root cause.

---

# 22. Bug Fix Report

After fixing a bug, report only:

```text
Root Cause:
...

Fix:
...

Verification:
...
```

Do not produce a long theoretical explanation unless requested.

---

# 23. Regression Test Rule

If a bug is caused by a reproducible logic problem:

> Add a regression test when practical.

Do not modify an existing test only to make it pass.

Never weaken an assertion just because the implementation currently fails it.

If a test is genuinely incorrect:

1. Explain why.
2. Fix the test.
3. Fix the implementation if necessary.
4. Run the complete relevant test set.

---

# 24. Refactoring Rules

Do not combine:

```text
feature
+
large refactor
+
dependency upgrade
+
architecture redesign
```

in one task unless explicitly requested.

Prefer:

```text
small change
→ test
→ stable
→ next change
```

A refactor is justified only when it directly improves the current task or removes a demonstrated source of bugs.

Do not refactor merely because another architecture looks cleaner.

---

# 25. Dependency Rules

Do not introduce a new dependency unless it provides a meaningful benefit.

Before adding one:

* check whether an existing dependency already solves the problem
* check whether the standard library is sufficient
* consider build size
* consider platform compatibility
* consider Tauri/WebView compatibility
* consider maintenance cost

Do not add a dependency for a trivial helper function.

---

# 26. Platform Rules

Gosslan targets:

* Windows
* macOS
* Android

When modifying platform-sensitive code:

* do not assume desktop-only behavior
* do not assume Windows-only APIs
* do not assume macOS-only APIs
* do not introduce browser-only behavior into Tauri core logic
* consider Android limitations when modifying shared Rust code

However:

> Do not over-engineer cross-platform abstractions before a real platform problem exists.

---

# 27. Performance Rules

Do not optimize based on theory alone.

First establish:

```text
Is there an actual performance problem?
```

Then:

```text
Measure
→ identify bottleneck
→ make focused optimization
→ measure again
```

Do not introduce:

* caches
* worker systems
* queues
* complex schedulers
* advanced routing
* custom concurrency

just because they might be faster.

Correctness comes first.

---

# 28. Testing Levels

## L1

Simple change:

```text
targeted test
```

## L2

Feature:

```text
targeted tests
+
npm test / cargo test where relevant
```

## L3

Core system:

```text
unit tests
+
integration tests
+
E2E
+
build
```

For message/network changes, prefer real protocol-path testing over mocks.

---

# 29. v0.12 Minimum Verification

At minimum:

```bash
npm test
npm run build

cd src-tauri
cargo test --lib
cargo check
```

For network/protocol/message changes:

```bash
cd src-tauri
cargo build --example e2e_peer
```

and:

```bash
bash scripts/e2e-dev.sh
```

When possible, verify with two actual devices on the same LAN.

---

# 30. Real Device Acceptance

Automated tests are not enough for Stable LAN Chat.

At least two real devices should verify:

```text
Discovery
↓
Friend
↓
A → B message
↓
B → A message
↓
100 messages
↓
Offline
↓
Outbox
↓
Reconnect
↓
Automatic resend
↓
ACK
↓
Read receipt
↓
Restart
↓
Continue chatting
↓
Image
↓
File
```

The final goal is:

> The chat works reliably in the real LAN environment.

---

# 31. Frozen Feature Rule

If a task is not required for current LAN Chat stability:

Do not proactively implement it.

If you notice a possible future improvement:

```text
Do not implement automatically.
```

Instead:

```text
Record it as a possible future improvement.
```

Do not let future requirements contaminate current implementation.

---

# 32. No Duplicate Logic

Before adding a function, ask:

```text
Does this responsibility already exist?
```

If yes:

```text
Reuse or modify it.
```

Do not create:

```text
sendMessage()
sendChatMessage()
sendReliableMessage()
sendP2PMessage()
sendNetworkMessage()
```

when they represent the same responsibility.

One clear path is preferred.

---

# 33. No Unnecessary Abstraction

Do not create:

```text
IMessageService
MessageServiceFactory
MessageTransportProvider
MessageRepositoryFactory
MessagePipelineCoordinator
```

unless there is a real current requirement for them.

The goal is understandable code.

Not maximum abstraction.

---

# 34. Change Scope

Every task should answer:

```text
What needs to change?
```

Prefer the smallest set of files that can correctly solve the problem.

Avoid unrelated:

* renaming
* formatting
* dependency upgrades
* directory restructuring
* code style rewrites

unless requested.

---

# 35. When AI Must Stop and Ask

Normally, continue implementing without unnecessary questions.

Stop and ask only when:

1. Requirement is genuinely ambiguous.
2. Two interpretations produce materially different behavior.
3. A security decision is required.
4. A destructive data operation is required.
5. A major architectural decision is unavoidable.
6. A new dependency is required but has meaningful trade-offs.
7. A breaking change affects a real released client.

Do not stop merely because:

```text
"there are several ways to implement this."
```

Choose the simplest correct implementation.

---

# 36. When AI Should NOT Ask

Do not ask for confirmation for:

* obvious UI fixes
* obvious bug fixes
* existing patterns
* straightforward refactors within the same module
* adding a regression test
* updating documentation to match implemented behavior
* small internal API changes
* pre-release protocol cleanup
* pre-release DB cleanup

Use engineering judgment.

---

# 37. Definition of Done

A task is complete when:

```text
Feature works
+
Relevant tests pass
+
No obvious regression
+
Existing invariants remain valid
+
No unnecessary architecture was introduced
```

For core networking tasks:

```text
Feature works
+
Unit tests pass
+
E2E passes
+
Message invariants remain valid
+
No silent failure path
```

Do not claim completion merely because:

```text
code compiles
```

---

# 38. Final Self-Check

Before reporting completion, ask:

### Scope

* Did I solve the requested problem?
* Did I implement anything that was not requested?

### Reuse

* Did I reuse existing logic?
* Did I accidentally create duplicate logic?

### Reliability

* Can the message be lost?
* Can it be duplicated?
* Can it remain stuck?
* Does retry behave correctly?
* Does reconnect behave correctly?

### Persistence

* Does restart preserve required state?

### Security

* Did I accidentally bypass E2EE?
* Did I introduce plaintext fallback?

### Testing

* Did I run the relevant tests?
* Did I add a regression test where appropriate?

### Complexity

* Did I make the solution more complicated than necessary?

If the answer to the last question is:

```text
Yes
```

simplify it before finishing.

---

# 39. Most Important Rule

When uncertain, follow this priority:

```text
Current user requirement
>
Current LAN Chat stability
>
Existing correct implementation
>
Core invariants
>
Simple maintainable solution
>
Future extensibility
>
Future roadmap
```

Never sacrifice a working current chat system to prepare for a feature that does not exist yet.

---

# 40. v0.12 Development Principle

The current phase is:

> **Stabilize first. Expand later.**

The AI should behave like an engineer maintaining a small, reliable LAN chat application.

Not like an architect trying to build the final distributed system in advance.

The desired behavior is:

```text
Understand enough
→
Reuse existing code
→
Make the smallest correct change
→
Test it
→
Move on
```

Not:

```text
Analyze everything
→
Redesign everything
→
Abstract everything
→
Implement future features
→
Create more complexity
```

---

# 41. v0.12 Success Criteria

v0.12 is successful when two real LAN devices can reliably:

```text
Discover each other
        ↓
Become friends
        ↓
Chat both directions
        ↓
Send text
        ↓
Send image
        ↓
Send file
        ↓
Receive ACK
        ↓
Read messages
        ↓
Go offline
        ↓
Queue messages
        ↓
Reconnect
        ↓
Automatically resend
        ↓
Avoid duplicates
        ↓
Restart application
        ↓
Continue chatting
```

without major crashes, message loss, duplicate messages, or state corruption.

That is the current definition of **Stable LAN Chat**.
