# Contributing to Depot

## Ground rules

- **Newcomers first.** Every label, button and error message should make sense to someone who has
  used Linux for a week. Say what will happen, not which command runs.
- **No bloat.** New dependencies need a good reason. The core crate depends only on `serde` and `serde_json`.
- **Never surprise the user with deletions.** Anything Depot removes is listed before it happens,
  and files outside a package manager go to the Trash.
- **Never finish silently.** Every change shows its progress and ends by saying what worked and
  what didn't.
- **Motion has a purpose.** Animate state changes (a page appearing, a tick, a result), keep it
  short (150 to 300 ms), and use GTK's own transitions or CSS so "Reduce animation" turns it off.

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
4. Add a `.badge-<name>` colour to `data/style.css`, then add its remove step in `steps()` in
   `src/ui/remove.rs`. Batch every app of that type into one command where the tool allows it,
   so selecting several apps means one password prompt, not one each. If it can remove other
   things along with the app, list them in the confirmation first.
5. For updates, add a check function to the backend (like `snap::refreshable`), match its results in
   `crates/depot-core/src/updates.rs`, call it from `check()` in `src/updates.rs`, and add a
   batch for it in `batches()` and `apply()` there.
6. Add a fixture test in `crates/depot-core/tests/inventory.rs`.

## Every change runs in the progress window

Anything that changes the computer goes through `src/ui/operation.rs`, never a silent background
task. Use `operation::command()` for a package-manager command (it shows the password step and
says when the password was accepted) and `operation::pk_reporter()` for PackageKit. End with
`Operation::finish()` and a result that says, in one plain sentence, what happened. Map new error
messages to plain language in `system::plain()`.

## Accessibility

- Give every icon-only button a spoken name with
  `update_property(&[gtk::accessible::Property::Label(...)])`, as well as a tooltip.
- Grid cards are read through the list item's label (`widgets::card_label`), so keep it in step
  with what the card shows.
- Anything that changes on its own, such as a progress step or a result, should go through
  `announce()`, the way `Operation` and the selection bar do.
- Still to check by hand: every screen at 360 px wide, and every screen with Orca
  (Super+Alt+S), including the uninstall, updates, progress and clean-up dialogs.

## Releases, metadata and legal notices

- Add the release to `<releases>` in `data/dev.kimzam.Depot.metainfo.xml` and bump `version` in
  `Cargo.toml`. CI checks the file with `appstreamcli validate`.
- After changing dependencies, run `./scripts/third-party.py` so the About window's licence list
  stays right.

## Seeing the app without a desktop

GTK's Broadway backend draws the app in a web browser, which is handy for screenshots:

```sh
gtk4-broadwayd :5 &
GDK_BACKEND=broadway BROADWAY_DISPLAY=:5 cargo run   # then open http://localhost:8085
```

Only one Depot runs at a time, so close the normal one first, or give the test copy its own session
bus with `dbus-run-session -- cargo run`. If everything looks squeezed, give
Broadway a DPI: start it with `XDG_CONFIG_HOME` pointing at a folder whose `gtk-4.0/settings.ini`
says `gtk-xft-dpi=98304`. Broadway's mouse input is unreliable with automation tools, so treat it
as a way to look at screens, not to script clicks.
