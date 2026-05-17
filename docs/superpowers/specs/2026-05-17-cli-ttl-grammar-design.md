# CLI TTL Grammar: Interactive Commands for Per-Key Expiry

**Date:** 2026-05-17
**Task:** [#56](../../../TASKS.md) — `TTL` command (CLI surface)
**Scope:** Define the human-typed grammar in the interactive client (`src/cli.rs` `parse_command`) for the per-key TTL feature: setting a TTL at write time, a post-hoc TTL setter / PERSIST, and a uniform-TTL bulk write. Pure parser concern — produces `Command` variants only.
**Out of scope:** The BFFP wire protocol and engine semantics (covered by [2026-05-10-ttl-design.md](2026-05-10-ttl-design.md)); per-entry mixed-TTL bulk writes in the interactive grammar (programmatic-only, served by the wire MSET extension §2.3); any time/clock logic in the CLI.

## Problem

The TTL feature ([2026-05-10-ttl-design.md](2026-05-10-ttl-design.md)) extends the `Command` enum (§10): `Write(String, String, Option<u32>)`, `Mset(Vec<(String, String, Option<u32>)>)`, and a new `Ttl(String, u32)`. The interactive client's `parse_command` ([src/cli.rs](../../../src/cli.rs)) currently constructs the old 2-field `Write`/`Mset` and has no TTL surface — the crate does not compile until these call sites are updated, and a human at the REPL has no way to express an expiry.

The central constraint: `WRITE key value` parses the value as `words[2..].join(" ")` — **space-greedy**. Any inline TTL token on `WRITE` is ambiguous with a value that contains that token. The grammar must be *unambiguous* (locked priority — chosen over Redis familiarity and over minimal change).

## Design

All changes are confined to `parse_command` in [src/cli.rs](../../../src/cli.rs). **No new `Command` variants, no dispatch/engine/wire changes** — the new verbs are alternate front-ends that emit the `Command` shapes already defined in the wire spec §10.

### Grammar

| Typed grammar | Emits | Rules |
|---|---|---|
| `WRITETTL <key> <seconds> <value...>` | `Command::Write(key, value, Some(seconds))` | ≥ 4 tokens. `key = words[1]`, `seconds = words[2]`, `value = words[3..].join(" ")` (space-greedy, preserved). `seconds` parses as `u32` and is **≥ 1**. |
| `TTL <key> <seconds>` | `Command::Ttl(key, seconds)` | Exactly 3 tokens. `seconds` parses as `u32`; **`0` is valid and means PERSIST** (strip TTL), per the wire spec's locked decision. |
| `MWRITETTL <seconds> k1 v1 k2 v2 …` | `Command::Mset(vec![(k, v, Some(seconds)), …])` | ≥ 4 tokens. `seconds = words[1]`, parses as `u32` and is **≥ 1** — applied uniformly to every pair. Pairs = `words[2..].chunks_exact(2)`. A trailing odd token → `InvalidInput` (stricter than legacy `MSET`). |

Unchanged grammar, updated only to satisfy the new `Command` arity:

- `WRITE <key> <value...>` → `Command::Write(key, value, None)`
- `MSET k1 v1 k2 v2 …` → `Command::Mset(vec![(k, v, None), …])` (legacy `chunks_exact(2)` + `filter_map` behavior preserved, including its silent drop of a trailing odd token — not changed here)

All other verbs (`READ`, `DELETE`, `EXISTS`, `MGET`, `RANGE`, `COMPACT`, `STATS`, `LIST`, `PING`, `QUIT`) are untouched.

### Why these shapes

- **`WRITETTL` (distinct verb, fixed prefix).** The verb signals the grammar; `seconds` sits at a fixed position *before* the free-form value, so `value = words[3..].join(" ")` stays space-greedy with zero ambiguity (e.g. `WRITETTL k 60 EX 5 TTL note` → value `"EX 5 TTL note"`). `WRITE` and its tests are untouched. Direct precedent: Redis `SETEX key seconds value`. Naming follows this project's `WRITE`/`READ`/`DELETE` verb dialect (chosen over the Redis `SETEX` spelling, which would import a `SET`-family verb).
- **`seconds ≥ 1` for the write verbs.** A write-with-TTL whose TTL is `0` is contradictory; reject with a usage error rather than silently meaning "no expiry" or "instantly dead." Matches Redis `SETEX` rejecting non-positive.
- **`TTL key 0` = PERSIST.** The post-hoc setter legitimately needs the "remove TTL" operation; `0` is the spec's locked PERSIST sentinel. The CLI only parses it; the `0 → strip` semantic lives at dispatch.
- **`MWRITETTL` uniform form.** One TTL for the whole batch matches the realistic interactive bulk case (e.g. a telemetry batch sharing one retention window). `seconds` fixed at `words[1]` reuses the existing `MSET` pair parsing; `MSET` values are single-token so there is no space ambiguity. Per-entry mixed TTL is intentionally excluded (see Decisions Locked).

### Interface boundary

`cli.rs` remains a **pure parser**: it selects the `Command` variant and validates arity + numeric form only. All TTL *semantics* — relative `seconds` → absolute `expiry_ms`, the `0 = PERSIST` sentinel, key-absent → `NotFound` — live at the dispatch boundary per wire spec §10. The CLI never reads a clock. Consequence: `parse_command` stays unit-testable with zero engine/time dependencies.

### Error handling

Each new verb emits a precise `Usage: …` string in the existing `parse_command` style on: insufficient arity, non-numeric `seconds`, `seconds == 0` for `WRITETTL`/`MWRITETTL`, or an odd trailing token for `MWRITETTL`. The `u32::MAX` ceiling falls out of `parse::<u32>()` for free, matching the wire spec's ~136-year cap.

## Test Plan

Parser-only unit tests for `parse_command` (no engine, no clock):

- `WRITETTL` happy path; value with a leading digit; value literally containing `TTL`/`EX`; → asserts `Command::Write(_, _, Some(n))`.
- `WRITETTL` rejects: < 4 tokens, non-numeric seconds, `seconds == 0`.
- `TTL` happy path → `Command::Ttl(k, n)`; `TTL k 0` → `Command::Ttl(k, 0)` (persist); rejects ≠ 3 tokens, non-numeric.
- `MWRITETTL` happy path (multi-pair, uniform `Some(seconds)` on every tuple); rejects `seconds == 0`, odd trailing token, < 4 tokens.
- Regression: `WRITE` → `Command::Write(_, _, None)`; `MSET` → tuples with `None`.

## Decisions Locked

| Decision | Choice | Rationale |
|---|---|---|
| Disambiguation strategy | Distinct verbs with fixed-position `seconds` | Only fully-unambiguous option; leaves space-greedy `WRITE` and its tests untouched |
| Write-with-TTL verb name | `WRITETTL` | Consistent with the project's `WRITE`/`READ`/`DELETE` dialect; `SETEX` rejected to avoid a `SET`-family verb |
| `WRITETTL`/`MWRITETTL` seconds = 0 | Reject (`InvalidInput`) | A TTL-bearing write with no TTL is contradictory; matches Redis `SETEX` |
| `TTL key 0` | PERSIST (valid) | Mirrors the wire spec's locked PERSIST sentinel |
| Bulk verb | Include `MWRITETTL`, uniform-TTL form | User decision (overrides industry convention); uniform matches the realistic interactive batch case |
| Per-entry mixed-TTL bulk | Excluded from interactive grammar | Programmatic pattern already served by wire MSET extension (§2.3); industry standard (Redis/Memcached/Cassandra) has no bulk-TTL verb at all — bulk-with-TTL is pipelining + per-key |
| CLI responsibility | Pure parser; semantics at dispatch | Keeps `parse_command` clock-free and unit-testable; single source of TTL semantics (§10) |

## Future Work

- **Per-entry mixed-TTL interactive bulk.** Not built. If a concrete interactive need appears, the natural shape is `MWRITETTLX k1 v1 t1 k2 v2 t2 …` (chunks of 3) mapping to the same `Command::Mset` with per-tuple `Some(ti)`. Recorded only; deferred per YAGNI and the industry-convention rationale above.
- **Quoted/escaped values.** The interactive grammar has no quoting, so a `WRITE` value cannot contain the exact byte sequence a future inline option would need. Out of scope; the distinct-verb design sidesteps it entirely for TTL.
