# Repository Guidelines

## Project Structure & Module Organization

This repository is a Rust 2021 Cargo workspace. Keep reusable application and domain logic in `crates/forge-core`; `crates/forge-cli` (`forge`) and `crates/forge-gui` (`forge-ide`) should remain thin adapters over that core. Core integration tests live in `crates/forge-core/tests`, with input data under `tests/fixtures`. Repository documentation is in `docs/`, JSON schemas in `schemas/`, and the runnable sample workspace is in `examples/demo-workspace`. GUI images and fonts belong in `crates/forge-gui/assets`.

## Build, Test, and Development Commands

- `cargo build --workspace` builds all crates in debug mode.
- `cargo build --release` creates `target/release/forge` and `target/release/forge-ide`.
- `cargo run -p forge-gui --bin forge-ide` starts the desktop IDE.
- `cargo run -p forge-cli -- run examples/demo-workspace` runs the sample workspace through the CLI.
- `cargo test --workspace` runs all unit and integration tests.
- `cargo fmt --all -- --check` verifies formatting.
- `cargo clippy --workspace --all-targets -- -D warnings` treats lint warnings as failures.

The GUI requires the Linux windowing development packages listed in `README.md`; core and CLI builds do not.

## Coding Style & Naming Conventions

Use `rustfmt` defaults and four-space indentation. Follow Rust conventions: `snake_case` for modules, functions, and files; `CamelCase` for structs, enums, and traits; `SCREAMING_SNAKE_CASE` for constants. Prefer focused modules and existing workspace dependencies. Keep protocol, storage, and UI concerns outside domain models where practical.

## Testing Guidelines

Add integration tests as `crates/forge-core/tests/<feature>_test.rs`; place stable sample inputs in `tests/fixtures/<feature>/`. Use `#[tokio::test]` for async paths and local test servers such as `wiremock` instead of external services. Run the focused test first (for example, `cargo test -p forge-core --test runner_test`), then the full workspace suite. No coverage threshold is currently configured.

## Commit & Pull Request Guidelines

Follow the existing concise, imperative history. Use a subsystem prefix when useful, such as `reqv1: validate sibling schemas` or `GUI: improve request editor`. Keep commits narrowly scoped. Pull requests should explain behavior changes, list executed checks, link relevant issues, and include screenshots for visible GUI changes. Never commit `.forge-local/`, `.forge/`, `.env.local`, or `*.secrets.json`.
