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

Matches on `Kind` have no catch-all arm on purpose: once you add the variant, the compiler lists
every place that has to decide what to do with it.

1. Add a variant to `Kind` in `crates/depot-core/src/model.rs`, add it to `Kind::ALL`, and fill in
   its `badge()`, `css()`, `source()` and `explain()` texts. Then follow the compiler errors.
2. Add `crates/depot-core/src/backend/<name>.rs` with a pure `parse()` that turns the tool's output
   into `Package`s, so it can be unit-tested with sample output, and a `load()` that runs the tool
   through `command::output()`. That returns `Ok(None)` when the tool isn't installed: return an
   empty list then, not an error. Two patterns to copy:
   - **A tool that knows what it installed** (Snap, pipx, Homebrew): ask it, like `backend/snap.rs`.
     If its own records are plain files, reading them is faster than running it (`backend/cargo.rs`
     reads `.crates2.json`, `backend/npm.rs` reads `package.json` files).
   - **Files with no package manager** (AppImage): look in a few known folders, like
     `backend/appimage.rs`, and add any folder that holds people's own files to `PERSONAL` in
     `crates/depot-core/src/remove.rs` so it's never trashed.
3. Add a `Source` for it in `sources()` in `crates/depot-core/src/inventory.rs`, which runs every
   source in parallel. Teach `claim()` to recognise its launchers, or fill in `Package::paths` so
   launchers pointing into those files are matched to it. A distro package manager goes in
   `inventory::Native` instead, with an `owners()` lookup built on `owners::cached` so file
   ownership is cached between runs.
4. Add a `.badge-<name>` colour to `data/style.css`.
5. Add its remove step to `steps()` in `src/ui/remove.rs` (a developer tool only needs a line in
   `tool_uninstall()`). Batch every app of that type into one command where the tool allows it,
   so selecting several apps means one password prompt, not one each. If it can remove other
   things along with the app, list them in the confirmation first.
6. For updates, add an outdated check to the backend (like `snap::refreshable` or
   `cargo::outdated`), turn its results into updates in `crates/depot-core/src/updates.rs`
   (`from_tool()` covers most tools), call it from `check()` in `src/updates.rs`, and give it a
   batch in `batches()` there.
7. Add a fixture test in `crates/depot-core/tests/inventory.rs`, and map the tool's error
   messages to plain language in `system::plain()`.

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
- Before a release, run [docs/ORCA-CHECKLIST.md](docs/ORCA-CHECKLIST.md) with the keyboard and
  Orca (Super+Alt+S). It can't be automated: GTK only talks to screen readers on a real display.
- Check that every screen still fits 360 px with the screenshot tour (below).

## Releases, metadata and legal notices

- Add the release to `<releases>` in `data/dev.kimzam.Depot.metainfo.xml` and bump `version` in
  `Cargo.toml`. The About window's "What's New" is read from that same entry, and a test fails if
  the newest release doesn't match the version. CI checks the file with `appstreamcli validate`.
- Store screenshots live in `docs/screenshots/` and are linked from the metainfo. Take them with
  the screenshot tour below.
- To publish a release, push a tag for the new version (`git tag v0.4.0 && git push origin v0.4.0`).
  `.github/workflows/release.yml` checks everything, builds the `.deb` and `.rpm` with `cargo deb`
  and `cargo generate-rpm`, and puts them on a GitHub Release with the notes from the metainfo.
  The tag has to match the version in `Cargo.toml`. To build the packages yourself:
  `cargo install cargo-deb cargo-generate-rpm`, then `cargo build --release --locked && cargo deb
  --no-build && cargo generate-rpm` (they land in `target/debian/` and `target/generate-rpm/`).
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

### The screenshot tour

Debug builds can visit every screen by themselves and save a picture of each:

```sh
cargo build
DEPOT_TOUR=/tmp/tour GDK_BACKEND=broadway BROADWAY_DISPLAY=:5 dbus-run-session -- target/debug/depot
```

It uses a 360 × 780 window (set `DEPOT_TOUR_SIZE=1024x720` for store screenshots), and writes
`report.txt` with how wide each screen needs to be. A screen over 360 px, or a dialog over 300 px
(dialogs keep 30 px of window on each side), doesn't fit a phone, and gets a `.why.txt` file
naming the widest part. Broadway only draws while a browser is watching, so keep
`http://localhost:8085` open (a headless Chrome works) while the tour runs.
