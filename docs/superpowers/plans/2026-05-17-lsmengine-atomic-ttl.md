# LsmEngine Atomic TTL via Lock/Logic Separation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `LsmEngine::ttl` an atomic read-modify-write by separating lock-free read/write logic into pure functions and orchestrating all locking in the public methods.

**Architecture:** Extract two pure, lock-free helpers — `lookup` (active → immutable → SSTable resolution with expiry/tombstone filtering) and `apply_write` (WAL append + memtable mutation). Helpers borrow state (`&Memtable`, `&mut Wal`, …), never `&self`, so they cannot lock — this makes std `RwLock`/`Mutex` re-entrancy structurally impossible. Public methods (`get`, `set_with_ttl`, `delete`, `ttl`) acquire locks in one canonical order, call the helpers, release, then trigger flush. `ttl` holds the lock span across both helpers, closing the lost-update / resurrection race.

**Tech Stack:** Rust, `std::sync::{RwLock, Mutex}`, existing `Memtable`/`Wal`/`StorageStrategy` types.

---

## Prerequisite (NOT a task in this plan)

The `56-ttl` branch is currently red on ~12 unrelated migration errors (kvengine trait impl, cli arity, etc.). **This plan's tests cannot run until the crate compiles.** Do not start this plan until `cargo check` is green. Fixing the migration is separate, out-of-scope work tracked under #56.

## Canonical Lock Order (the load-bearing invariant)

Derived from `compact` ([src/lsmengine.rs:256-269](../../../src/lsmengine.rs#L256)). **Every multi-lock site MUST acquire in this order:**

```
flush_handle  →  wal  →  storage_strategy  →  active  →  immutable
```

- `get` today acquires `active`/`immutable`/`storage_strategy` *sequentially* (acquire-use-drop) — compatible (no nesting).
- `set_with_ttl`/`delete` acquire `wal → active` — a compatible subset.
- `ttl` (new) acquires `wal.lock() → storage_strategy.read() → active.write() → immutable.read()` — held simultaneously across lookup+write.
- Acquisition order ≠ usage order. Once all guards are held, helpers may *use* them in any logical order (e.g. lookup reads active, then immutable, then strategy).

Getting this wrong trades a re-entrancy deadlock for an intermittent lock-order deadlock against `compact` — strictly worse. Task 6 adds a deadlock regression test.

## File Structure

- **Modify:** `src/lsmengine.rs` — add `lookup` + `apply_write` (free functions or `impl LsmEngine` assoc. fns taking borrowed state, NOT `&self`); rewrite `get`, `set_with_ttl`, `delete`, `ttl` as orchestration.
- **Create:** `tests/lsm_ttl_atomic.rs` — unit tests for the helpers + concurrency/deadlock regression tests.
- **Modify:** `docs/superpowers/specs/2026-05-10-ttl-design.md` — replace the "TTL is best-effort / not atomic" stance with the atomic design + canonical lock order note.

Helper signatures (lock-free; the compiler enforces no `&self`):

```rust
fn lookup(
    active: &Memtable,
    immutable: Option<&Memtable>,
    strategy: &dyn StorageStrategy,
    key: &str,
    now_ms: u64,
) -> io::Result<Option<String>>;

// Returns new memtable size_bytes so the caller can decide to flush AFTER releasing locks.
fn apply_write(
    wal: &mut Wal,
    active: &mut Memtable,
    key: &str,
    value: Option<&str>,   // None = tombstone (delete)
    expiry_ms: Option<u64>,
) -> io::Result<usize>;
```

---

### Task 1: Extract `lookup` (lock-free read chunk)

**Files:**
- Modify: `src/lsmengine.rs` (add `lookup`; do not change `get` yet)
- Test: `tests/lsm_ttl_atomic.rs`

- [ ] **Step 1: Write the failing test**

```rust
// tests/lsm_ttl_atomic.rs
use rustikv::lsmengine::lookup;          // expose via `pub(crate)` or a test-only `pub`
use rustikv::memtable::Memtable;

#[test]
fn lookup_active_hides_expired_without_falling_through() {
    let mut active = Memtable::new();
    active.insert("k".into(), "new".into(), Some(1_000)); // expired at now=2_000
    let mut older = Memtable::new();
    older.insert("k".into(), "old".into(), None);          // stands in for an SSTable hit

    // Active holds the newest version; expired => None, NO fall-through.
    let got = lookup(&active, Some(&older), /*empty strategy*/ &empty_strategy(), "k", 2_000).unwrap();
    assert_eq!(got, None);
}
```

(Provide an `empty_strategy()` test helper returning a `Box<dyn StorageStrategy>` with no segments, or use the existing strategy constructor with an empty dir.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test lsm_ttl_atomic lookup_active_hides_expired -- --nocapture`
Expected: FAIL — `lookup` not found / unresolved import.

- [ ] **Step 3: Write minimal implementation**

Move the resolution logic currently inlined in `get` ([src/lsmengine.rs:161-219](../../../src/lsmengine.rs#L161)) into `lookup`, taking borrowed state instead of locking `self.shared`. Preserve exactly: active expiry => `None` with no fall-through ([spec §5](../../specs/2026-05-10-ttl-design.md#L166)); immutable layer; SSTable layer via `strategy.iter_for_key`. Mark `pub(crate)` (or `#[cfg(test)] pub`) so the test can call it.

Preserve the immutable-layer semantics exactly as they now stand at [src/lsmengine.rs:183-202](../../../src/lsmengine.rs#L183): immutable **miss** (`entry == None`) falls through to the SSTable layer; immutable **tombstone** (`value == None`) returns `Ok(None)` with no fall-through; immutable **expired** hit returns `Ok(None)` with no fall-through (same shadowing rationale as the active memtable in spec §5 — the immutable memtable is newer than any SSTable). These are correct intended behaviors, not bugs to "clean up." (The earlier `None => todo!()` panic on this arm has already been fixed to `None => {}` — do not reintroduce a panic here.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test lsm_ttl_atomic lookup_active_hides_expired`
Expected: PASS

- [ ] **Step 5: Add coverage + commit**

Add tests: immutable-hit, SSTable-hit, tombstone-in-active, expired-in-immutable. Then:

```bash
git add src/lsmengine.rs tests/lsm_ttl_atomic.rs
git commit -m "#56 — Extract lock-free lookup() from LsmEngine::get"
```

---

### Task 2: Refactor `get` to orchestrate + delegate to `lookup`

**Files:**
- Modify: `src/lsmengine.rs:161-219`
- Test: existing `tests/lsmengine.rs`, `tests/lsm_ttl.rs` (no new test — behavior must be unchanged)

- [ ] **Step 1: Run existing get/ttl tests (baseline, must already pass)**

Run: `cargo test --test lsmengine --test lsm_ttl`
Expected: PASS (record the list of passing tests)

- [ ] **Step 2: Rewrite `get` as orchestration**

Acquire in canonical order (`active.read()` then, only if needed, `immutable.read()`, `storage_strategy.read()` — sequential acquire-use-drop is fine for `get` since it doesn't need them simultaneously), capture `now_ms` once, call `lookup`. No behavior change.

- [ ] **Step 3: Run the same tests to verify unchanged**

Run: `cargo test --test lsmengine --test lsm_ttl`
Expected: PASS — identical set to Step 1.

- [ ] **Step 4: Commit**

```bash
git add src/lsmengine.rs
git commit -m "#56 — Route LsmEngine::get through lock-free lookup()"
```

---

### Task 3: Extract `apply_write` (lock-free write chunk)

**Files:**
- Modify: `src/lsmengine.rs` (add `apply_write`; do not change callers yet)
- Test: `tests/lsm_ttl_atomic.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn apply_write_sets_value_and_expiry_in_memtable() {
    let mut wal = test_wal();          // helper: fresh Wal in a tempdir
    let mut active = Memtable::new();
    let sz = apply_write(&mut wal, &mut active, "k", Some("v"), Some(9_999)).unwrap();
    let e = active.entry("k").unwrap();
    assert_eq!(e.value.as_deref(), Some("v"));
    assert_eq!(e.expiry_ms, Some(9_999));   // expiry MUST reach the memtable
    assert!(sz > 0);
}

#[test]
fn apply_write_none_value_is_tombstone() {
    let mut wal = test_wal();
    let mut active = Memtable::new();
    apply_write(&mut wal, &mut active, "k", None, None).unwrap();
    assert!(active.entry("k").unwrap().value.is_none());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test lsm_ttl_atomic apply_write`
Expected: FAIL — `apply_write` not found.

- [ ] **Step 3: Write minimal implementation**

```rust
pub(crate) fn apply_write(
    wal: &mut Wal,
    active: &mut Memtable,
    key: &str,
    value: Option<&str>,
    expiry_ms: Option<u64>,
) -> io::Result<usize> {
    match value {
        Some(v) => {
            wal.append(key.to_string(), v.to_string(), false, expiry_ms)?;
            active.insert(key.to_string(), v.to_string(), expiry_ms);
        }
        None => {
            wal.append(key.to_string(), String::new(), true, None)?;
            active.remove(key.to_string());
        }
    }
    Ok(active.size_bytes())
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test lsm_ttl_atomic apply_write`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/lsmengine.rs tests/lsm_ttl_atomic.rs
git commit -m "#56 — Extract lock-free apply_write() for memtable+WAL writes"
```

---

### Task 4: Route `set_with_ttl` + `delete` through `apply_write` (fixes the dropped-expiry bug)

**Files:**
- Modify: `src/lsmengine.rs:430-444` (`set_with_ttl`), `src/lsmengine.rs:235-251` (`delete`)
- Test: `tests/lsm_ttl.rs` (add expiry-reaches-memtable assertion)

- [ ] **Step 1: Write the failing test**

```rust
// tests/lsm_ttl.rs
#[test]
fn set_with_ttl_expires_without_restart() {
    let (engine, _dir) = new_lsm_engine();
    engine.set_with_ttl("k", "v", Some(now_ms())).unwrap(); // already-expired instant
    std::thread::sleep(std::time::Duration::from_millis(5));
    assert_eq!(engine.get("k").unwrap(), None); // currently FAILS: stale `None` passed to insert
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test lsm_ttl set_with_ttl_expires_without_restart`
Expected: FAIL — key still readable because `set_with_ttl` passes `None` (current bug at [lsmengine.rs:435](../../../src/lsmengine.rs#L435)).

- [ ] **Step 3: Rewrite both methods as orchestration**

`set_with_ttl`: acquire `wal.lock()` → `active.write()` (canonical subset), call `apply_write(.., Some(value), expiry_ms)`, drop guards, then `if size >= max_memtable_bytes { flush_memtable_async() }`. `delete`: same, `apply_write(.., None, None)`; preserve the `exists` check semantics ([lsmengine.rs:236](../../../src/lsmengine.rs#L236)). **Flush trigger stays outside the held memtable lock.**

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test --test lsm_ttl --test lsmengine`
Expected: PASS including the new test.

- [ ] **Step 5: Commit**

```bash
git add src/lsmengine.rs tests/lsm_ttl.rs
git commit -m "#56 — Route set_with_ttl/delete through apply_write; fix dropped expiry"
```

---

### Task 5: Rewrite `ttl` as an atomic read-modify-write

**Files:**
- Modify: `src/lsmengine.rs:414-428`
- Test: `tests/lsm_ttl.rs` (outcome cases)

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn ttl_outcomes() {
    let (engine, _dir) = new_lsm_engine();
    assert!(matches!(engine.ttl("missing", Some(now_ms()+10_000)).unwrap(), TtlOutcome::NotFound));
    engine.set("k", "v").unwrap();
    assert!(matches!(engine.ttl("k", Some(now_ms()+10_000)).unwrap(), TtlOutcome::Set));
    assert!(matches!(engine.ttl("k", None).unwrap(), TtlOutcome::Persisted));
}
```

- [ ] **Step 2: Run to verify it fails or passes**

Run: `cargo test --test lsm_ttl ttl_outcomes`
Expected: PASS likely (mechanics already work) — this pins behavior before the locking rewrite so Step 4 proves no regression.

- [ ] **Step 3: Rewrite `ttl` to hold one lock span**

```rust
fn ttl(&self, key: &str, expiry_ms: Option<u64>) -> io::Result<TtlOutcome> {
    let now = now_ms();
    let (found, size) = {
        // Canonical order: wal -> storage_strategy -> active -> immutable
        let mut wal      = self.shared.wal.lock().unwrap();
        let strategy     = self.shared.storage_strategy.read().unwrap();
        let mut active   = self.shared.active.write().unwrap();
        let immutable    = self.shared.immutable.read().unwrap();

        match lookup(&active, immutable.as_ref(), strategy.as_ref(), key, now)? {
            Some(v) => {
                let sz = apply_write(&mut wal, &mut active, key, Some(&v), expiry_ms)?;
                (true, Some(sz))
            }
            None => (false, None),
        }
        // all guards dropped here
    };
    if !found { return Ok(TtlOutcome::NotFound); }
    if let Some(sz) = size {
        if sz >= self.shared.max_memtable_bytes.load(Relaxed) {
            self.flush_memtable_async()?;
        }
    }
    Ok(if expiry_ms.is_some() { TtlOutcome::Set } else { TtlOutcome::Persisted })
}
```

Note: `lookup` returning `None` for an already-expired key preserves the spec semantic "TTL on an expired key → NotFound" ([spec §2.1](../../specs/2026-05-10-ttl-design.md#L53)).

- [ ] **Step 4: Run tests to verify unchanged**

Run: `cargo test --test lsm_ttl --test lsmengine`
Expected: PASS (same outcomes; now atomic).

- [ ] **Step 5: Commit**

```bash
git add src/lsmengine.rs
git commit -m "#56 — Make LsmEngine::ttl an atomic read-modify-write"
```

---

### Task 6: Concurrency + deadlock regression tests

**Files:**
- Test: `tests/lsm_ttl_atomic.rs`

- [ ] **Step 1: Lost-update / resurrection stress test**

```rust
#[test]
fn ttl_does_not_lose_concurrent_writes() {
    let (engine, _dir) = new_lsm_engine();
    engine.set("k", "v0").unwrap();
    let e = std::sync::Arc::new(engine);
    let w = { let e=e.clone(); std::thread::spawn(move || {
        for i in 0..2000 { e.set("k", &format!("v{i}")).unwrap(); } }) };
    let t = { let e=e.clone(); std::thread::spawn(move || {
        for _ in 0..2000 { let _ = e.ttl("k", Some(now_ms()+60_000)); } }) };
    w.join().unwrap(); t.join().unwrap();
    // Invariant: final value is SOME writer value, never resurrected/empty.
    assert!(e.get("k").unwrap().is_some());
}
```

Probabilistic by nature — document that. The deterministic correctness guarantee is the single lock span in Task 5; this test is a smoke screen against regressions.

- [ ] **Step 2: Deadlock regression test (validates canonical order vs `compact`)**

```rust
#[test]
fn ttl_and_compact_do_not_deadlock() {
    let (engine, _dir) = new_lsm_engine();
    for i in 0..200 { engine.set(&format!("k{i}"), "v").unwrap(); }
    let e = std::sync::Arc::new(engine);
    let c = { let e=e.clone(); std::thread::spawn(move || { for _ in 0..50 { let _=e.compact(); } }) };
    let t = { let e=e.clone(); std::thread::spawn(move || { for _ in 0..500 { let _=e.ttl("k1", Some(now_ms()+1_000)); } }) };
    // If the lock order is wrong, this hangs and the CI timeout kills it.
    c.join().unwrap(); t.join().unwrap();
}
```

- [ ] **Step 3: Run**

Run: `cargo test --test lsm_ttl_atomic -- --test-threads=4`
Expected: PASS, completes well under the default timeout (no hang).

- [ ] **Step 4: Commit**

```bash
git add tests/lsm_ttl_atomic.rs
git commit -m "#56 — Add TTL atomicity + lock-order deadlock regression tests"
```

---

### Task 7: Update the TTL design spec

**Files:**
- Modify: `docs/superpowers/specs/2026-05-10-ttl-design.md`

- [ ] **Step 1: Replace the atomicity stance**

Add a new "Concurrency" subsection (the current spec has no atomicity language to replace — this is additive): document that `ttl` is an atomic read-modify-write achieved via lock/logic separation, state the canonical lock order, and note `lookup`/`apply_write` are the lock-free primitives.

- [ ] **Step 2: Commit**

```bash
git add docs/superpowers/specs/2026-05-10-ttl-design.md
git commit -m "#56 — Spec: document atomic TTL + canonical lock order"
```

---

### Task 8: Final verification (rustikv pre-commit gate)

- [ ] **Step 1:** `cargo fmt --all` then `cargo clippy --all-targets -- -D warnings`
- [ ] **Step 2:** `cargo test` (full suite) — record pass counts; confirm no flakiness over 3 runs of `lsm_ttl_atomic`.
- [ ] **Step 3:** Invoke the `rustikv-pre-commit` skill before the final commit/PR per repo convention.
- [ ] **Step 4:** Update `TASKS.md` per AGENTS.md workflow if this is tracked as its own task; otherwise note completion under #56.

---

## Out of Scope

- The KV engine's `ttl` (separate engine, separate locking model).
- The size-tiered partial-merge tombstone GC (#82 — already filed).
- Fixing the ~12 unrelated migration compile errors (prerequisite, tracked under #56).
- A `Clock` trait for time control (TTL design Future Work #81).
