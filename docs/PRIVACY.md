# Privacy

*Last updated: 27 October 2026*

Depot is a program that runs on your own computer. It has no accounts, no ads, no analytics and no
tracking, and it doesn't send anything about you or your computer to its developer or anyone else.

## What Depot reads

To show what's installed, Depot reads your computer's own records: the package databases of your
system's package manager (dpkg, rpm or pacman), Snap and Flatpak, the developer tools' own lists
(cargo, npm, pipx, Homebrew and Nix, when they're installed), and the app launchers in your app
menu. To find AppImages, it also looks at the names of the files in a few folders in your home
folder (`~/Applications`, `~/AppImages`, `~/Downloads`, `~/Desktop`, `~/bin` and `~/.local/bin`),
without opening your personal files. It does this every time it starts or you
press **Refresh**. All of it stays on your computer.

Depot keeps a small cache in `~/.cache/depot` so it can start faster, and one setting in
`~/.config/depot` (whether it has shown its first-uninstall note). Removing Depot with
`./install.sh --uninstall` deletes both.

## When Depot uses the internet

Only to **check for and install updates**, and only through the tools your computer already uses for
that: PackageKit (which contacts your distribution's software sources), Snap (the Snap Store),
Flatpak (usually Flathub), and, if you use them, cargo (crates.io), npm (the npm registry) and
Homebrew. These are the same requests the Software app makes. What those services
log is covered by their own privacy policies. Depot adds nothing about you to these requests.

Opening an app's **Website** link, or **Report a Problem** in the menu, opens your web browser.

## Reports you make

`depot --list-only` saves a list of what's installed to a file on your computer. It goes nowhere
unless you choose to share it, for example by attaching it to a bug report. That file contains app
and package names and the folders they're installed in, which can include your username. It
doesn't contain your personal files or what's in them.

The **Troubleshooting** page in the About window shows the Depot version, your Linux version and
how many apps of each kind are installed. It's only shared if you copy or save it yourself.

## Questions

Open an issue at <https://github.com/kimzam30/Nhako-Depot/issues>.
