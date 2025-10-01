# Contributing

Thank you for your interest in improving asfship! This document outlines the expectations for development workflow, coding standards, and validation before submitting changes.

## Getting Started
- Use the Rust stable toolchain defined in `rust-toolchain.toml`. Install it via `rustup` if necessary (the file also enables `rustfmt` and `clippy`).
- Run `cargo check` to ensure the workspace builds before making substantial edits.
- Keep code and comments in English; user-facing discussions may be localized externally.

## Development Workflow
1. Create a focused branch for your change and keep commits logically grouped.
2. Prefer small, reviewable pull requests. When implementing multi-step features, land them incrementally with tests and documentation updates.
3. When you touch release logic, update the relevant templates and docs under `docs/` if behavior changes.
4. Reflect architectural decisions or constraints in `AGENTS.md` so future maintainers inherit the context.

## Quality Gates
Before opening a pull request, run the following commands and ensure they succeed:

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo cca
```

`cargo cca` performs conventional commit analysis and should pass when commit metadata follows the expected format. If the binary is missing locally, install it via `cargo install cargo-cca` or consult the maintainers for alternatives.

## Testing Guidance
- Add unit tests for new parsing, versioning, or planning logic. Use fixtures under `tests/fixtures` when practical.
- Avoid introducing network-dependent tests; mock GitHub and SVN interactions through the existing abstraction layers.
- Run targeted tests with `cargo test <module>::<case>` during development, then execute the full suite before submission.

## Coding Standards
- Follow idiomatic Rust patterns and keep modules small and cohesive.
- Remove unused code rather than suppressing warnings; never add `#[allow(dead_code)]`.
- Only add comments when behavior is non-obvious, and keep them concise.

## Documentation
- Update `README.md` and the docs under `docs/` when user-visible behavior changes.
- Record new limitations, rollout plans, or architectural notes in `AGENTS.md`.
- Ensure Markdown remains ASCII-only unless existing content already uses extended characters.

## Reviews and Merging
- Respond promptly to review feedback and reference follow-up issues when you defer improvements.
- Respect the maintainer’s merge strategy (squash or rebase) and keep commit messages meaningful.
- Coordinate with maintainers before cutting releases to confirm environment credentials and checklist status.
