# Bug Fix Record

> 用于 Gosslan 所有非 trivial Bug。
>
> 原则：
>
> **先证明怎么复现，再证明根因，再证明不会回来。**

---

## 1. Basic

- Bug ID:
- Date:
- Reporter:
- Version:
- Platform:
  - [ ] Windows
  - [ ] macOS
  - [ ] Android
- Severity:
  - [ ] P0 Data loss / security
  - [ ] P1 Core feature broken
  - [ ] P2 Degraded behavior
  - [ ] P3 UI / cosmetic

---

## 2. Symptom

### Actual

```text
```

### Expected

```text
```

---

## 3. Reproduction

### Preconditions

```text
```

### Steps

```text
1.
2.
3.
4.
```

### Reproduction rate

```text
例如：3/10、稳定复现、仅 macOS
```

---

## 4. Data Flow

写出实际链路：

```text
UI
→ Store
→ Tauri
→ Rust
→ Protocol
→ Transport
→ Peer
→ DB
→ Event
→ UI
```

标记故障点：

```text
                              ↓ ROOT CAUSE
UI → Store → Command → Rust → Protocol → Transport
```

---

## 5. Root Cause

> 必须写“为什么”，不能只写“哪里”。

### Root Cause

```text
```

### Why did it happen?

```text
```

### Why did existing tests not catch it?

```text
```

---

## 6. Invariants

检查：

- [ ] INV-P01 Message ID
- [ ] INV-P02 Idempotency
- [ ] INV-P03 ACK
- [ ] INV-P04 Outbox
- [ ] INV-P05 Crash safety
- [ ] INV-P06 Retry
- [ ] INV-P07 Gossip convergence
- [ ] INV-P10 E2EE
- [ ] INV-P12 Identity
- [ ] INV-P15 Event / UI
- [ ] Other: ______

---

## 7. Fix

### Changed files

```text
```

### Minimal fix

```text
```

### Why this fix is correct

```text
```

### Why not other approaches?

```text
```

---

## 8. Regression Test

### Test name

```text
```

### Test type

- [ ] Unit
- [ ] Protocol
- [ ] Integration
- [ ] E2E
- [ ] Two-device
- [ ] Manual platform test

### Test scenario

```text
```

### Expected

```text
```

---

## 9. Failure Matrix

| Failure | Expected behavior | Tested |
|---|---|---|
| Network disconnect |  | [ ] |
| ACK missing |  | [ ] |
| Duplicate |  | [ ] |
| Restart |  | [ ] |
| Peer offline |  | [ ] |
| Key missing |  | [ ] |
| Key changed |  | [ ] |
| Other |  | [ ] |

---

## 10. Verification

```bash
npm test
npm run build

cd src-tauri
cargo test --lib
cargo check
```

Additional:

```text
```

### Result

```text
```

---

## 11. Documentation

- [ ] CHANGELOG updated
- [ ] AI_PROJECT_HANDOFF updated
- [ ] ADR updated
- [ ] Protocol invariants updated
- [ ] README updated
- [ ] No documentation change required

---

## 12. Final Review

- [ ] Root cause fixed, not symptom
- [ ] Regression test added
- [ ] No new duplicate logic
- [ ] No protocol bypass
- [ ] No outbox bypass
- [ ] No ACK semantic change
- [ ] No silent error
- [ ] No unrelated refactor
- [ ] Cross-platform impact checked

---

## 13. One-line Summary

```text
[ROOT CAUSE] → [FIX] → [REGRESSION TEST]
```
