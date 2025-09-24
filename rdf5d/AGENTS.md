# Repository Guidelines

## Project Structure & Module Organization
- Root crate: `Cargo.toml` (Rust edition 2024).
- Library code lives in `src/` (entry: `src/lib.rs`). Keep modules small and cohesive (e.g., `src/codec/`, `src/dict/`).
- Unit tests sit next to code via `#[cfg(test)]` blocks; add integration tests under `tests/` when behavior spans modules.
- Architecture reference: see `ARCH.md` for on‑disk format details and terminology.

## Build, Test, and Development Commands
- Build: `cargo build` — compile debug build; `cargo build --release` for optimized.
- Test: `cargo test` — run unit/integration tests.
- Format: `cargo fmt --all` — apply `rustfmt` to the whole workspace.
- Lint: `cargo clippy --all-targets -- -D warnings` — static checks; treat warnings as errors.

## Coding Style & Naming Conventions
- Indentation: 4 spaces; no tabs. Always run `cargo fmt` before committing.
- Naming: `snake_case` for modules/files/functions; `CamelCase` for types/traits; `SCREAMING_SNAKE_CASE` for constants.
- Error handling: prefer `Result<T, E>` with descriptive error enums; avoid panics in library code.
- Docs: add `///` rustdoc on public items; include small examples where practical.

## Testing Guidelines
- Unit tests colocated with modules for tight feedback; name tests after behavior (e.g., `encodes_empty_dict`).
- Integration tests in `tests/` mirror public API surface; one file per feature area (e.g., `tests/dict_roundtrip.rs`).
- Edge cases: cover empty inputs, max sizes, and invalid headers described in `ARCH.md`.
- Run: `cargo test`; aim to keep tests deterministic and mmap‑safe (no flaky fs assumptions).

## Commit & Pull Request Guidelines
- Messages: use Conventional Commits (e.g., `feat(dict): add key16 index`, `fix(io): handle LE offset overflow`).
- PRs: include a clear description, link related issues, reference `ARCH.md` sections when relevant, add/adjust tests, and attach benchmarks if performance‑affecting.
- Pre‑PR checklist: `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test` all passing.

## Security & Configuration Tips
- Use stable Rust with up‑to‑date toolchain via `rustup`; enable `mmap`‑related tests only on supported platforms.
- When touching file I/O or offsets, validate sizes and bounds; prefer checked math and fuzz tests for parsers.
