<p align="center">
  <img src="docs/brand/logo-circle.png" alt="Depot logo: a pixel-art penguin in a yellow scarf, smiling, on a pink circle" width="128">
</p>

<h1 align="center">Depot</h1>

**See every app on your Linux computer in one place. Keep them up to date, and remove the ones you don't need.**

Linux software comes from many places: your distro's packages, Snap, Flatpak, downloaded archives,
AppImages and more. Depot puts all of it in one window, tells you in plain language how each app was
installed, and updates or removes it the right way, so you don't need to know which command to use.

Depot is made for people who are new to Linux, and it's fast: a full scan of about 2,500 packages
takes around 50 ms, from a single native program of about 1 MB.

## What it does

- **Shows everything installed** as **Deb** (apt), **RPM** (dnf), **Pacman** (including AUR builds),
  **Snap**, **Flatpak** or **Manual** (installed by hand), each with a badge that explains what it means.
- **Gets the source right.** Each app appears once, under the package it really came from, even
  when an install script copied its launcher into place. An app installed more than one way (say
  as a Deb and a Flatpak) gets one card that shows every copy, so you can keep just one.
- **Updates.** Lists newer versions of your apps with their old and new version numbers. Update
  them one at a time or all at once. Libraries, drivers and runtimes are grouped into a single
  "System updates" entry.
- **Removes apps safely.** Before anything is removed, Depot shows exactly what will go with it.
  Leftover packages that nothing needs any more are listed separately, and you can keep them.
  Apps installed by hand are moved to the **Trash**, so you can undo.
- **Finds broken launchers**: menu entries left behind after their program was removed.
- **Keeps the clutter out of the way.** Libraries, command-line tools and runtimes live under
  **Advanced**, so the main view shows only your apps.

New to Depot? The **[step-by-step guide](docs/GUIDE.md)** walks you through everything.

## Install

Depot needs GTK 4.14 or newer and libadwaita 1.5 or newer: Ubuntu 24.04, Zorin OS 18, Linux Mint 22,
Debian 13, Fedora 40, Arch Linux, or anything newer. Debian-based systems are the most tested; Fedora
and Arch support is new, so please [report](docs/GUIDE.md#reporting-a-problem) anything that looks
wrong there. On Fedora, updating and removing system packages goes through PackageKit, which Fedora
includes. There are no ready-made downloads yet, so you build it once from source. It takes about
five minutes.

**1. Install the build tools** (asks for your password):

```sh
# Ubuntu, Zorin OS, Linux Mint, Debian
sudo apt install git libgtk-4-dev libadwaita-1-dev pkg-config build-essential curl

# Fedora
sudo dnf install git gtk4-devel libadwaita-devel gcc curl

# Arch Linux
sudo pacman -S --needed git gtk4 libadwaita base-devel curl
```

**2. Install Rust**, if you don't have it. Choose the default option when asked, then close and reopen
the terminal:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

(Don't use `sudo apt install cargo`: the version in Ubuntu 24.04 is too old for Depot.)

**3. Download Depot and install it:**

```sh
git clone https://github.com/kimzam30/Nhako-Depot.git
cd Nhako-Depot
./install.sh
```

Depot is installed just for you, in `~/.local`, so this step doesn't need your password. Open it
from your app menu by searching for **Depot**.

**Updating Depot itself:** go back to that folder and run `git pull && ./install.sh`.

**Removing Depot:** run `./install.sh --uninstall` from the same folder.

## Roadmap

- [x] Rust rewrite, same GTK 4 / libadwaita design
- [x] Correct ownership detection (launcher copies, `Exec` targets, XDG overrides)
- [x] Updates page with **Update** and **Update all** (PackageKit, Snap, Flatpak)
- [x] Removal through PackageKit with progress and clean-up of unused dependencies
- [x] One card per app even when it's installed several ways
- [x] RPM (dnf, zypper) and Pacman (including AUR packages)
- [ ] AppImage
- [ ] Developer tools: pipx, cargo, npm, Homebrew, Nix
- [ ] `.deb` and `.rpm` downloads on GitHub Releases

## For developers

```sh
cargo test -p depot-core   # the engine's tests; doesn't need GTK installed
cargo run --release        # run the app
```

`depot --help` lists commands that work without opening a window, handy for bug reports:
`--list-only` prints everything installed as JSON, `--updates` prints available updates, and
`--simulate-remove NAME` / `--simulate-update` show what PackageKit would do without changing anything.

| Path | What's there |
| --- | --- |
| `crates/depot-core` | Inventory engine with no GTK dependency: one backend per install type, launcher ownership, finding apps installed twice, update matching, removal planning, caching |
| `src/packagekit.rs` | Small async PackageKit client over D-Bus |
| `src/updates.rs` | Checking every source for updates and installing them |
| `src/system.rs` | Running package-manager commands (with pkexec when needed) |
| `src/ui` | GTK 4 / libadwaita interface |
| `data/` | Stylesheet, launcher, icon |
| `docs/` | The beginner's guide and the logo |

Performance comes from reading package databases directly (no `dpkg-query` per package), loading
all sources in parallel, and caching file-ownership lookups until the package database changes.

## Contributing

Issues and pull requests are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
