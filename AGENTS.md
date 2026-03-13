# Agent Instructions

This is a didactic Bitcask-style key-value store built while reading *Designing Data-Intensive Applications*. The project is educational — simplicity and clarity matter more than production-readiness.

## Task Workflow

- Tasks live in `TASKS.md` at the repo root, split into **Open Tasks** and **Closed Tasks**.
- Before starting work, read `TASKS.md` and identify the relevant task number.
- When a task is done, move its entire `## #N` section from **Open Tasks** to **Closed Tasks** — never delete it.
- New tasks get the next sequential `#N` number.

## Git & PRs

- **Never push directly to `main`** — always use a dedicated branch and open a PR.
- Use **git** and the **gh CLI** for all version control and PR operations.
- Branch off `main` with the pattern `<task-number>-<short-description>` (e.g. `15-crc-checksums`).
- PR title format: `#<task-number> — <short description>`.
- PR body must include a line: `Opened via <agent>` (e.g. `Opened via Copilot`, `Opened via Claude`, `Opened via OpenCode`).
- Keep PRs focused on a single task.
- After opening a PR, add the PR link to the corresponding task in `TASKS.md`.
- Before opening the PR, move the task from **Open Tasks** to **Closed Tasks** in `TASKS.md`.
- After completing a task, always ask the user if they want to checkout back to `main`.

## Agent Role

- **Do not implement features or write production code unless the user explicitly asks.**
- The user writes the implementation — the agent assists with testing, reviewing, and running checks.
- When a task involves code changes, propose an approach and wait for the user to confirm or ask for implementation.
- Proactively write and run tests, run `cargo clippy`, and review code for correctness.

## Code Style

- Rust, built with Cargo. Source in `src/`, integration tests in `tests/`.
- Run `cargo fmt` after editing code — all code must be formatted.
- Run `cargo clippy -- -D warnings` — treat all clippy warnings as errors.
- Run `cargo test` before committing — all unit and integration tests must pass.
- Prefer hand-rolled implementations over external crates when the goal is to learn the concept.
- Follow existing patterns and module structure. New modules go in `src/`.
- Keep implementations simple. Avoid over-engineering or premature abstraction.

## Project Structure

| Path | Purpose |
|------|---------|
| `src/main.rs` | TCP server, command parsing, request handling |
| `src/db.rs` | DB struct — get/set/delete/compact operations |
| `src/record.rs` | On-disk record format (header + key + value) |
| `src/hash_index.rs` | In-memory hash map index (key → byte offset) |
| `src/segment.rs` | Segment file naming, parsing, listing |
| `src/settings.rs` | CLI argument parsing |
| `src/stats.rs` | Atomic runtime counters |
| `tests/` | Integration tests (TCP-level) |
