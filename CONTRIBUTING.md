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
cargo run --release -p depot-core --example scan -- --summary   # the inventory, also without GTK
cargo fmt --all
```

These need the GTK development files (`sudo apt install libgtk-4-dev libadwaita-1-dev`):

```sh
cargo clippy --workspace --all-targets -- -D warnings
cargo run --release               # the app
cargo run --release -- --list-only | less
```

## Adding a new install type

1. Add a variant to `Kind` in `crates/depot-core/src/model.rs`, and fill in its `badge()`, `css()`,
   `source()` and `explain()` texts.
2. Add `crates/depot-core/src/backend/<name>.rs` with a `load()` that returns `Result<Vec<Package>, String>`.
   If the tool isn't installed, return `Ok(vec![])`, not an error. Keep the parsing in a separate
   `parse()` function so it can be unit-tested with sample output.
3. Call it from `inventory::scan`, which runs backends in parallel, and teach `claim()` how to
   recognise that type's launchers. A distro package manager instead goes in `inventory::Native`,
   with an `owners()` lookup built on `owners::cached` so file ownership is cached between runs.
4. Add a `.badge-<name>` colour to `data/style.css`, then add the remove command in `src/ui/remove.rs`.
   If it can remove other things along with the app, list them in the confirmation first.
5. For updates, add a check function to the backend (like `snap::refreshable`), match its results in
   `crates/depot-core/src/updates.rs`, and call it from `check()` and `apply()` in `src/updates.rs`.
6. Add a fixture test in `crates/depot-core/tests/inventory.rs`.
