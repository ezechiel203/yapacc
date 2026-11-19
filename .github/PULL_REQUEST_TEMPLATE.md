<!-- What this changes and why, in a few sentences. Link the issue if there is one. -->

## How it was checked

<!-- The commands you ran and what they showed. `cargo xtask ci` runs the
     CI jobs on your machine; `cargo xtask ci portable` is the quick part. -->

- [ ] `cargo fmt --all` and `cargo clippy --workspace --all-targets -- -D warnings` are clean
- [ ] `cargo test --workspace --all-targets` passes
- [ ] a change in behaviour has a test, and the documentation that describes it is updated
- [ ] no file under `fixtures/` was edited by hand (they are captures; see `fixtures/README.md`)

By opening this pull request you agree that your contribution is licensed
under GPL-2.0-only, the licence of the project.
