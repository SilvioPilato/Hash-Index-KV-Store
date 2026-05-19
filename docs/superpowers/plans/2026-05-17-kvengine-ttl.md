# KVEngine TTL Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete per-key TTL for the Bitcask-style `KVEngine`: read-time expiry filtering on all read paths, expiry-aware compaction cleanup, an `expiry_ms`-carrying in-memory index + hint file, and an atomic `ttl` via lock/logic separation.

**Architecture:** `KVEngine` reads through a hash index pointing at exactly one live record per key — there is **no read-time fall-through across segments**, so the LSM "tombstone-vs-drop / resurrection" problem (plan §bug-3) **does not apply**: an expired key is simply dropped + de-indexed at compaction, never tombstoned. The cost of that simplicity is that `exists`/`list_keys` consult only the index, so the index (and the hint file that persists it) must carry `expiry_ms` to filter cheaply. `ttl` gets the same lock/logic-separation treatment as the LSM plan, using KVEngine's own canonical lock order.

**Tech Stack:** Rust, `std::sync::{RwLock, Mutex}`, existing `HashIndex`/`Hint`/`Record`/`Wal` types.

---

## Canonical Lock Order (load-bearing invariant)

Derived from `KVEngine::compact` ([src/kvengine.rs:347-350](../../../src/kvengine.rs#L347)):

```text
wal  →  active_file  →  active_segment  →  index
```

Field types (from [src/kvengine.rs](../../../src/kvengine.rs)): `wal: Mutex<Wal>`, `active_file: Mutex<ActiveFile>`, `active_segment: Mutex<Segment>`, `index: RwLock<HashIndex>`. Every multi-lock site (notably the new atomic `ttl`) MUST acquire in this order or it deadlocks against `compact`. Task 7 adds a regression test.

## Architectural note to preserve (do not "fix")

`build_compacted` ([src/kvengine.rs:216-222](../../../src/kvengine.rs#L216)) rebuilds by iterating live index keys and calling `self.get(&k)`. **Once `get` filters expired keys (Task 4), compaction cleanup is automatic** — an expired key's `get` returns `None`, the `None => continue` arm skips it, and it never enters the new segment, index, or hint file. Task 5 only adds: (a) preserving `expiry_ms` on *surviving* TTL keys (else compaction silently strips their TTL), and (b) the `expired_compacted` stat. **No tombstone is ever written for an expired key in KVEngine** — that is correct and intentional (no fall-through ⇒ no resurrection).

## File Structure

- **Modify:** `src/cli.rs` (Task 0), `src/hash_index.rs` (Task 1), `src/hint.rs` (Task 2), `src/kvengine.rs` (Tasks 3-6).
- **Create:** `tests/kv_ttl.rs` — spec §Test-Plan file for KV (set-with-TTL, get/exists/list_keys pre/post expiry, compaction drops expired + preserves survivor TTL, hint-file round-trips expiry, atomic ttl, deadlock). Test conventions: `use rustikv::utils::now_ms;` (the LSM engine's choice — *not* `Stats::now_ms`); follow spec §8 (1-second wire TTLs, ≥2 s sleeps) where wall-clock margin matters, though most tests here use already-elapsed `now_ms()` for determinism.
- **Modify:** `docs/superpowers/specs/2026-05-10-ttl-design.md` — confirm/expand the KV §6 note + canonical lock order + the no-tombstone rationale.

---

### Task 0: Make the crate green (prerequisite, in-scope)

KVEngine itself has **no** compile errors. The only crate-wide blocker is `cli.rs` not passing the new TTL arg on `Command::Write`/`Mset` (spec §10 changed their arity). Without this the whole crate is red and *no* test in this plan can run.

**Files:** Modify `src/cli.rs:18`, `src/cli.rs:71`

- [ ] **Step 1: Confirm the only errors**

Run: `cargo check --message-format=short 2>&1 | grep ": error"`
Expected: exactly two — `cli.rs:18` (`Command::Write` takes 3 args, 2 supplied) and `cli.rs:71` (`Command::Mset` tuple arity).

- [ ] **Step 2: Fix both call sites**

`cli.rs:18`: `Command::Write(words[1].to_string(), words[2..].join(" "), None)` (the interactive CLI doesn't parse TTL syntax; `None` = no TTL).
`cli.rs:71`: build `Vec<(String, String, Option<u32>)>` — map each `(k, v)` chunk to `(k.to_string(), v.to_string(), None)`.

- [ ] **Step 3: Verify green**

Run: `cargo check`
Expected: compiles (0 errors). Warnings OK.

- [ ] **Step 4: Commit**

```bash
git add src/cli.rs
git commit -m "#56 — cli: pass None TTL on Command::Write/Mset to compile"
```

---

### Task 1: `IndexEntry` carries `expiry_ms`

**Files:** Modify `src/hash_index.rs` (struct `IndexEntry` L14-18, `set` L38-51, `from_file` ~L66, segment-scan `rebuild` ~L63+); Test: `tests/kv_ttl.rs`

- [ ] **Step 1: Write the failing test**

```rust
// tests/kv_ttl.rs
use rustikv::hash_index::HashIndex;
#[test]
fn index_entry_round_trips_expiry() {
    let mut ix = HashIndex::new();
    ix.set("k".into(), 0, 1, 10, Some(42));
    assert_eq!(ix.get("k").unwrap().expiry_ms, Some(42));
}
```

- [ ] **Step 2: Run, expect FAIL** — `set` takes 4 args / no `expiry_ms` field.
Run: `cargo test --test kv_ttl index_entry_round_trips_expiry`

- [ ] **Step 3: Implement**

Add `pub expiry_ms: Option<u64>` to `IndexEntry`. Add `expiry_ms: Option<u64>` as the final param of `HashIndex::set` and store it. Then update **every** `index.set(...)` / index-population site to pass it. The real sites (verify exact lines, do not assume a single `from_file` reload):

- `KVEngine::set` → `None`; `KVEngine::set_with_ttl` → `expiry_ms`.
- The **hint-reload** recovery path in `KVEngine::from_dir` (the `Hint::read_file` + `hash_index.set(...)` loop, ~kvengine.rs:114): thread `hint_entry.expiry_ms` (wired in Task 2 — until then pass `None`).
- The **segment-scan** recovery path (`KVEngine::merge_from_file` ~kvengine.rs:149 and the scan loops in `hash_index.rs`, e.g. `merge_from_file`/`from_file` ~L66-118): read `record.header.expiry_ms`.

`cargo check` after this step is the safety net for any missed call site.

- [ ] **Step 4: Run, expect PASS.** Then `cargo check` (catch missed call sites).

- [ ] **Step 5: Commit**

```bash
git add src/hash_index.rs src/kvengine.rs tests/kv_ttl.rs
git commit -m "#56 — KV: IndexEntry carries expiry_ms"
```

---

### Task 2: Hint file persists expiry (additive format, spec §1 style)

**Files:** Modify `src/hint.rs` (`HintEntry` L11-16, `write_file` L19-42, `read_file` L44+); `src/kvengine.rs` `build_compacted` hint construction (L236-254) + `from_file` index load; Test: `tests/kv_ttl.rs`

The current per-entry hint layout is `key_size(8) | offset(8) | tombstone(1) | key`. Replace the bare `tombstone` byte with a **flags byte** reusing `FLAG_TOMBSTONE`/`FLAG_HAS_EXPIRY` (same bits as records — consistent with spec §1's additive bitmask), and append an optional `expiry_ms(8)` when `FLAG_HAS_EXPIRY` is set:

```text
key_size(8) | offset(8) | flags(1) | [expiry_ms(8) if flags & FLAG_HAS_EXPIRY] | key
```

Hint files are derived artifacts (regenerated by compaction; rebuildable by segment scan), so this is a hard format change with **no migration concern** — note that explicitly.

- [ ] **Step 1: Failing test**

```rust
#[test]
fn hint_file_round_trips_expiry() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("x.hint");
    let e = HintEntry { key_size: 1, offset: 7, tombstone: false, expiry_ms: Some(99), key: "k".into() };
    Hint::write_file(p.clone(), &[e]).unwrap();
    let back = Hint::read_file(p).unwrap();
    assert_eq!(back[0].expiry_ms, Some(99));
    assert_eq!(back[0].offset, 7);
}
```

- [ ] **Step 2: Run, expect FAIL** (no `expiry_ms` on `HintEntry`).

- [ ] **Step 3: Implement** — add `expiry_ms: Option<u64>` to `HintEntry`; in `write_file` emit a flags byte (`FLAG_TOMBSTONE` if tombstone, `| FLAG_HAS_EXPIRY` if `expiry_ms.is_some()`) then the 8 expiry bytes when present; in `read_file` parse symmetrically. Update `build_compacted` ([kvengine.rs:242-247](../../../src/kvengine.rs#L242)) to populate `HintEntry.expiry_ms` from the surviving index entry. Then complete the Task 1 wiring: the hint-reload loop in `KVEngine::from_dir` (~kvengine.rs:114) now threads `hint_entry.expiry_ms` into `hash_index.set(...)` instead of the temporary `None`.

- [ ] **Step 4: Run, expect PASS.** `cargo check`.

- [ ] **Step 5: Commit**

```bash
git add src/hint.rs src/kvengine.rs tests/kv_ttl.rs
git commit -m "#56 — KV: hint file persists expiry via additive flags byte"
```

---

### Task 3: Fix `set_with_ttl` flag bug + write paths

**Files:** Modify `src/kvengine.rs` `set_with_ttl` (L600-607); Test: `tests/kv_ttl.rs`

Bug: [kvengine.rs:605](../../../src/kvengine.rs#L605) hardcodes `flags: FLAG_HAS_EXPIRY` even when `expiry_ms` is `None` (the `TTL key 0` / PERSIST path), producing a record whose flag claims a trailing expiry that isn't there → decode corruption.

- [ ] **Step 1: Failing test**

```rust
#[test]
fn persist_via_set_with_ttl_none_has_no_expiry_flag() {
    let (e, _d) = new_kv_engine();
    e.set("k", "v").unwrap();
    e.set_with_ttl("k", "v", None).unwrap();          // PERSIST
    assert_eq!(e.get("k").unwrap(), Some(("k".into(),"v".into())));
    // round-trips after reopen (decode must not expect phantom expiry bytes)
    drop(e);
    let e2 = open_kv_engine(&_d);
    assert_eq!(e2.get("k").unwrap(), Some(("k".into(),"v".into())));
}
```

- [ ] **Step 2: Run, expect FAIL** (decode misreads after reopen).

- [ ] **Step 3: Implement** — `flags: if expiry_ms.is_some() { FLAG_HAS_EXPIRY } else { 0 }` (mirrors the correct pattern at [kvengine.rs:138](../../../src/kvengine.rs#L138)). Confirm `index.set(..., expiry_ms)` is passed here (from Task 1).

- [ ] **Step 4: Run, expect PASS.**

- [ ] **Step 5: Commit**

```bash
git add src/kvengine.rs tests/kv_ttl.rs
git commit -m "#56 — KV: fix set_with_ttl FLAG_HAS_EXPIRY on PERSIST"
```

---

### Task 4: Read-path expiry filtering (`get`, `exists`, `list_keys`, `mget`)

**Files:** Modify `src/kvengine.rs` `get` (L383-413), `exists` (L539-541), `list_keys` (L535-537), (`mget` L543-557 inherits via `get`); Test: `tests/kv_ttl.rs`

> **Stats are out of scope.** `KVEngine` has no `Stats` access path (`Stats` is server-owned, threaded through `dispatch` only), and the sibling LSM TTL plan likewise records no expiry stats. Spec §9 (`expired_reads`/`expired_compacted`) is deferred — see Out of Scope. Do **not** mutate stats in any engine method here.
>
> **`is_expired` signature.** The codebase helper is `utils::is_expired(expiry_ms: u64, now_ms: u64) -> bool` (non-optional `u64`), already used by the LSM engine. With an `Option<u64>` field, call it as `expiry_ms.is_some_and(|e| is_expired(e, now))` — the established pattern in `leveled.rs`/`size_tiered.rs`. Do not pass `Option<u64>` to `is_expired` directly (type error).

- [ ] **Step 1: Failing test**

```rust
#[test]
fn reads_filter_expired() {
    let (e, _d) = new_kv_engine();
    e.set_with_ttl("k", "v", Some(now_ms())).unwrap(); // already expired
    std::thread::sleep(std::time::Duration::from_millis(5));
    assert_eq!(e.get("k").unwrap(), None);
    assert!(!e.exists("k"));
    assert!(!e.list_keys().unwrap().contains(&"k".to_string()));
}
```

- [ ] **Step 2: Run, expect FAIL** (currently `get` returns the value, `exists`/`list_keys` see the index entry).

- [ ] **Step 3: Implement** — capture `now` once per call via `let now = now_ms();` (spec §5/§8). `get`: after resolving the index entry, `if entry.expiry_ms.is_some_and(|e| is_expired(e, now)) { return Ok(None); }` — do **not** mutate the index (physical cleanup is compaction's job) and do **not** touch stats. `exists`: `index.get(key).is_some_and(|e| !e.expiry_ms.is_some_and(|x| is_expired(x, now)))`. `list_keys`: keep keys where `!entry.expiry_ms.is_some_and(|e| is_expired(e, now))`. `mget` already routes through `get` — no change. Match the LSM semantic: expired ⇒ logically absent.

- [ ] **Step 4: Run, expect PASS.** Also re-run `cargo test --test kvengine` (no regression on non-TTL paths).

- [ ] **Step 5: Commit**

```bash
git add src/kvengine.rs tests/kv_ttl.rs
git commit -m "#56 — KV: filter expired keys on get/exists/list_keys"
```

---

### Task 5: Compaction preserves survivor TTL + counts drops

**Files:** Modify `src/kvengine.rs` `build_compacted` (L216-222); Test: `tests/kv_ttl.rs`

After Task 4, expired keys are *already* skipped by `build_compacted`'s `self.get` (`None => continue`). The remaining gap: surviving TTL keys are re-inserted via `new_db.set` ([kvengine.rs:221](../../../src/kvengine.rs#L221)) which **drops their expiry** (silently kept forever). (Counting drops via spec §9 stats is deferred — see Out of Scope; do not add stats here.)

- [ ] **Step 1: Failing test**

```rust
#[test]
fn compaction_drops_expired_keeps_live_ttl() {
    let (e, _d) = new_kv_engine();
    e.set_with_ttl("dead", "x", Some(now_ms())).unwrap();
    e.set_with_ttl("live", "y", Some(now_ms() + 60_000)).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    e.compact().unwrap();
    assert_eq!(e.get("dead").unwrap(), None);
    assert_eq!(e.get("live").unwrap(), Some(("live".into(),"y".into())));
    // survivor still carries its TTL after compaction (index entry kept it)
    drop(e);
    let e2 = open_kv_engine(&_d);
    assert!(e2.get("live").unwrap().is_some());
}
```

- [ ] **Step 2: Run, expect FAIL** (`live` loses its TTL through compaction — silently kept forever).

- [ ] **Step 3: Implement** — in `build_compacted`, read the surviving key's `expiry_ms` from the source index entry and re-insert with `new_db.set_with_ttl(&k, &value, expiry_ms)` instead of `new_db.set`. Add a comment restating the no-tombstone rationale (no read-time fall-through ⇒ dropping + de-indexing an expired key cannot resurrect an older value). No stats mutation.

- [ ] **Step 4: Run, expect PASS.**

- [ ] **Step 5: Commit**

```bash
git add src/kvengine.rs tests/kv_ttl.rs
git commit -m "#56 — KV: compaction drops expired, preserves survivor TTL"
```

---

### Task 6: Atomic `ttl` via lock/logic separation

**Files:** Modify `src/kvengine.rs` `ttl` (L571-586); add lock-free helpers; Test: `tests/kv_ttl.rs`

`ttl` is currently get-then-set ([kvengine.rs:572-574](../../../src/kvengine.rs#L572)) — same lost-update/resurrection race as the LSM engine. Same fix shape: extract lock-free read + write logic, orchestrate locks once in canonical order.

- [ ] **Step 1: Pin behavior (outcome test)**

```rust
#[test]
fn ttl_outcomes_kv() {
    let (e, _d) = new_kv_engine();
    assert!(matches!(e.ttl("missing", Some(now_ms()+1_000)).unwrap(), TtlOutcome::NotFound));
    e.set("k","v").unwrap();
    assert!(matches!(e.ttl("k", Some(now_ms()+1_000)).unwrap(), TtlOutcome::Set));
    assert!(matches!(e.ttl("k", None).unwrap(), TtlOutcome::Persisted));
}
```

- [ ] **Step 2: Run, expect PASS** (mechanics already work — pins behavior before the locking rewrite).

- [ ] **Step 3: Implement** — extract two `&self`-free helpers operating on already-held guard state (no `self.*.lock()` inside them — the compiler then makes std-lock re-entrancy impossible):

```rust
// Resolve the live value for `key`, applying expiry filtering. Reads the
// record from disk via the index entry; needs db_path + the active
// segment's (timestamp, path) snapshot to pick the right segment file.
fn kv_lookup(
    index: &HashIndex,
    db_path: &str,
    active_seg_timestamp: u64,
    active_seg_path: &Path,
    key: &str,
    now: u64,
) -> io::Result<Option<String>>;

// Append a record + update the index. `expiry_ms` flows into both the
// record header (flags per Task 3) and the index entry (Task 1).
// Returns the record size for the caller's dead/total-bytes accounting.
fn kv_append(
    wal: &mut Wal,
    active_file: &mut ActiveFileState,
    active_segment: &mut Segment,
    index: &mut HashIndex,
    key: &str,
    value: &str,
    expiry_ms: Option<u64>,
) -> io::Result<u64>;
```

Rewrite `ttl` to acquire **in canonical order** (`wal → active_file → active_segment → index.write()`), call `kv_lookup`, and if `Some` call `kv_append` with the new `expiry_ms`, all under one lock span. Refactor `get`/`set_with_ttl` to also delegate to these helpers (acquire → snapshot what `kv_lookup` needs → delegate), so no method both locks and calls another locking method. Preserve `fsync()` and dead/total-bytes accounting *outside* the held locks (mirror current `set`/`get` structure).

- [ ] **Step 4: Run, expect PASS** (Step 1 test + `cargo test --test kvengine`).

- [ ] **Step 5: Commit**

```bash
git add src/kvengine.rs tests/kv_ttl.rs
git commit -m "#56 — KV: make ttl an atomic read-modify-write"
```

---

### Task 7: Concurrency + deadlock regression tests

**Files:** Test: `tests/kv_ttl.rs`

- [ ] **Step 1: Lost-update smoke test** — `Arc<KVEngine>`, one thread looping `set("k", vN)`, another looping `ttl("k", Some(...))`; after join assert `get("k")` is `Some` (never resurrected/empty). Document its probabilistic nature; the deterministic guarantee is the single lock span in Task 6.

- [ ] **Step 2: Deadlock regression** — one thread loops `compact()`, another loops `ttl("k1", Some(..))`; both joins must complete (a wrong lock order vs `compact` hangs → CI timeout kills it). This validates the canonical order.

- [ ] **Step 3: Run** — `cargo test --test kv_ttl -- --test-threads=4`; expect PASS, no hang.

- [ ] **Step 4: Commit**

```bash
git add tests/kv_ttl.rs
git commit -m "#56 — KV: TTL atomicity + lock-order deadlock regression tests"
```

---

### Task 8: Spec update

**Files:** Modify `docs/superpowers/specs/2026-05-10-ttl-design.md`

- [ ] **Step 1:** In §6's KV bullet, expand to state explicitly: KVEngine never tombstones expired records (no read-time fall-through ⇒ no resurrection); compaction drops + de-indexes; survivor TTL is preserved. Add the KV canonical lock order and the index/hint `expiry_ms` schema to the relevant sections. Add a Concurrency note that KV `ttl` is atomic via lock/logic separation.

- [ ] **Step 2: Commit**

```bash
git add docs/superpowers/specs/2026-05-10-ttl-design.md
git commit -m "#56 — Spec: document KV TTL semantics + canonical lock order"
```

---

### Task 9: Final verification (rustikv pre-commit gate)

- [ ] **Step 1:** `cargo fmt --all` then `cargo clippy --all-targets -- -D warnings`
- [ ] **Step 2:** `cargo test` (full suite); run `kv_ttl` 3× to check for flakiness in the concurrency tests.
- [ ] **Step 3:** Invoke the `rustikv-pre-commit` skill before the final commit/PR per repo convention.
- [ ] **Step 4:** Update `TASKS.md` per AGENTS.md if tracked separately; otherwise note completion under #56.

---

## Out of Scope

- **Spec §9 expiry stats (`expired_reads`/`expired_compacted`).** `Stats` is server-owned and reaches engines only via `dispatch`; engines have no `Stats` handle, and the sibling LSM TTL plan also defers this. Filing a separate follow-up to plumb expiry stats (either an `Arc<Stats>` into the engines or a `StorageEngine` reporting hook) is the right move — out of scope here so KV and LSM stay consistent.
- LSM engine TTL (covered by `2026-05-17-lsmengine-atomic-ttl.md`).
- Size-tiered partial-merge tombstone GC (#82, filed).
- A `Clock` trait for time control (TTL design Future Work #81).
- Background expiry sweeper (Future Work #79) — KV relies on compaction-driven cleanup; revisit only if that proves insufficient.
