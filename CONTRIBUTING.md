# Contributing to Depot

## Ground rules

- **Newcomers first.** Every label, button and error message should make sense to someone who has
  used Linux for a week. Say what will happen, not which command runs.
- **No bloat.** New dependencies need a good reason. The core crate depends only on `serde` and `serde_json`.
- **Never surprise the user with deletions.** Anything Depot removes is listed before it happens,
  and files outside a package manager go to the Trash.

## Development

```sh
cargo test -p depot-core          # fast; doesn't need GTK installed
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo run --release               # the app
cargo run --release -- --list-only | less
```

## Adding a new install type

1. Add a variant to `Kind` in `crates/depot-core/src/model.rs`, with its badge, source text and explanation.
2. Add `crates/depot-core/src/backend/<name>.rs` with a `load()` that returns `Result<Vec<Package>, String>`.
   If the tool isn't installed, return `Ok(vec![])`, not an error. Keep the parsing in a separate
   `parse()` function so it can be unit-tested with sample output.
3. Call it from `inventory::scan`, which runs backends in parallel, and teach `claim()` how to
   recognise that type's launchers.
4. Add a `.badge-<name>` colour to `data/style.css`, then add the remove command in `src/ui/remove.rs`.
5. Add a fixture test in `crates/depot-core/tests/inventory.rs`.
