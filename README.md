# Depot

**See every app on your Linux computer in one place, and remove the ones you don't need.**

Linux software comes from many places: your distro's packages, Snap, Flatpak, downloaded archives,
AppImages and more. Depot puts all of it in one window, tells you how each app was installed in
plain language, and removes it the right way, so you don't have to know which command to use.

Depot is built for people who are new to Linux, and it is fast: a full scan of about 2,500 packages
takes around 50 ms, from a single native binary under 1 MB.

## What it does today

- Lists everything installed as **Deb** (apt), **Snap**, **Flatpak** or **by hand**.
- Works out which package an app really belongs to, including launchers that install scripts copy
  into place, so each app appears once with the right source.
- Finds **broken launchers**: menu entries left behind after their program was removed.
- Uninstalls through the right tool: apt, snap or flatpak. Before removing a Deb, it shows which
  other packages would go with it. Hand-installed apps are **moved to the Trash**, so you can undo.
- Keeps libraries and runtimes under **Advanced**, so the main view shows only your apps.

## Roadmap

- [x] Rust rewrite, same GTK 4 / libadwaita design
- [x] Correct ownership detection (launcher copies, `Exec` targets, XDG overrides)
- [ ] One card per app even when it's installed several ways
- [ ] Updates page: see available updates, then **Update** or **Update all** (PackageKit, Snap, Flatpak)
- [ ] RPM (dnf, zypper) and Pacman (including AUR packages)
- [ ] AppImage
- [ ] Developer tools: pipx, cargo, npm, Homebrew, Nix
- [ ] `.deb` and `.rpm` downloads on GitHub Releases

## Build from source

Requirements: GTK 4.14+, libadwaita 1.5+ (Ubuntu 24.04, Zorin OS 18, Fedora 40 or newer) and a Rust toolchain.

```sh
# Debian / Ubuntu / Zorin
sudo apt install libgtk-4-dev libadwaita-1-dev pkg-config build-essential
# Fedora
sudo dnf install gtk4-devel libadwaita-devel

cargo build --release
./target/release/depot
```

`depot --list-only` prints the full inventory as JSON without opening a window. It's handy for bug reports.

## How it's built

| Path | What's there |
| --- | --- |
| `crates/depot-core` | Inventory engine with no GTK dependency: one backend per install type, launcher ownership, caching |
| `src/ui` | GTK 4 / libadwaita interface |
| `data/` | Stylesheet, launcher, icon |

Performance comes from reading package databases directly (no `dpkg-query` per package), loading
all sources in parallel, and caching file-ownership lookups until the package database changes.

## Contributing

Issues and pull requests are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
