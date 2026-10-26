use serde::Serialize;
use std::path::PathBuf;

/// How a piece of software got onto the computer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Deb,
    Rpm,
    Pacman,
    Snap,
    Flatpak,
    AppImage,
    Manual,
    /// `cargo install`
    Cargo,
    /// `npm install -g`
    Npm,
    Pipx,
    /// Homebrew on Linux
    Brew,
    /// `nix profile install`
    Nix,
}

impl Kind {
    /// Every kind, in the order the About page counts them.
    pub const ALL: [Kind; 12] = [
        Kind::Deb,
        Kind::Rpm,
        Kind::Pacman,
        Kind::Snap,
        Kind::Flatpak,
        Kind::AppImage,
        Kind::Manual,
        Kind::Cargo,
        Kind::Npm,
        Kind::Pipx,
        Kind::Brew,
        Kind::Nix,
    ];

    /// The distro's own package format (Deb, RPM or Pacman), managed through PackageKit.
    pub fn is_native(self) -> bool {
        matches!(self, Kind::Deb | Kind::Rpm | Kind::Pacman)
    }

    /// Installed with a developer tool's own package manager, which also removes and updates it.
    pub fn is_dev_tool(self) -> bool {
        matches!(self, Kind::Cargo | Kind::Npm | Kind::Pipx | Kind::Brew | Kind::Nix)
    }

    /// Removed by moving its files to the Trash rather than through a package manager.
    pub fn trashes(self) -> bool {
        matches!(self, Kind::Manual | Kind::AppImage)
    }

    /// Short text for the badge on a card.
    pub fn badge(self) -> &'static str {
        match self {
            Kind::Deb => "Deb",
            Kind::Rpm => "RPM",
            Kind::Pacman => "Pacman",
            Kind::Snap => "Snap",
            Kind::Flatpak => "Flatpak",
            Kind::AppImage => "AppImage",
            Kind::Manual => "Manual",
            Kind::Cargo => "Cargo",
            Kind::Npm => "npm",
            Kind::Pipx => "pipx",
            Kind::Brew => "Homebrew",
            Kind::Nix => "Nix",
        }
    }

    /// CSS suffix for `.badge-*`.
    pub fn css(self) -> &'static str {
        match self {
            Kind::Deb => "deb",
            Kind::Rpm => "rpm",
            Kind::Pacman => "pacman",
            Kind::Snap => "snap",
            Kind::Flatpak => "flatpak",
            Kind::AppImage => "appimage",
            Kind::Manual => "manual",
            Kind::Cargo => "cargo",
            Kind::Npm => "npm",
            Kind::Pipx => "pipx",
            Kind::Brew => "brew",
            Kind::Nix => "nix",
        }
    }

    /// Plain-language name of the source, for the details list.
    pub fn source(self) -> &'static str {
        match self {
            Kind::Deb => "Debian package (apt)",
            Kind::Rpm => "RPM package (dnf)",
            Kind::Pacman => "Pacman package",
            Kind::Snap => "Snap",
            Kind::Flatpak => "Flatpak",
            Kind::AppImage => "AppImage (a single downloaded file)",
            Kind::Manual => "Manual install (not from a package manager)",
            Kind::Cargo => "Rust program (cargo install)",
            Kind::Npm => "Node.js package (npm install -g)",
            Kind::Pipx => "Python app (pipx)",
            Kind::Brew => "Homebrew package",
            Kind::Nix => "Nix profile package",
        }
    }

    /// One sentence for newcomers, shown as the badge tooltip.
    pub fn explain(self) -> &'static str {
        match self {
            Kind::Deb => "Installed from your system's software sources with apt.",
            Kind::Rpm => "Installed from your system's software sources with dnf.",
            Kind::Pacman => "Installed with pacman, from your system's repositories or the AUR.",
            Kind::Snap => "Installed from the Snap Store. Snaps update themselves.",
            Kind::Flatpak => "Installed with Flatpak, usually from Flathub. Runs in its own sandbox.",
            Kind::AppImage => "A single downloaded file that runs the app. Removing it moves the file to the Trash.",
            Kind::Manual => {
                "Installed by hand (downloaded archive, script or launcher), not through a package manager."
            }
            Kind::Cargo => "Built from source and installed with Rust's cargo, usually from crates.io.",
            Kind::Npm => "Installed for the whole computer or your account with Node.js's npm.",
            Kind::Pipx => "A Python program installed with pipx, in a folder of its own.",
            Kind::Brew => "Installed with Homebrew, a package manager many developers use on Linux too.",
            Kind::Nix => "Installed into your Nix profile with the Nix package manager.",
        }
    }
}

/// What the package is to the user.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Has a launcher in the app menu.
    App,
    /// Library, command-line tool or other non-app package.
    #[default]
    Tool,
    /// Shared runtime or base that other packages need.
    Runtime,
}

/// Files belonging to a manually installed app.
#[derive(Clone, Debug, Default, Serialize)]
pub struct ManualPlan {
    /// The app's own folder, if it lives in one Depot recognises (e.g. ~/.local/opt/<app>).
    pub appdir: Option<PathBuf>,
    /// Program files and launchers that make up the install.
    pub files: Vec<PathBuf>,
    /// Settings and data folders, removed only on request.
    pub data: Vec<PathBuf>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Package {
    pub kind: Kind,
    pub role: Role,
    /// Identifier passed to the package manager (deb name, snap name, flatpak ref, launcher id).
    pub id: String,
    pub name: String,
    pub title: String,
    pub version: String,
    pub summary: String,
    pub description: String,
    /// Installed size in bytes, 0 if unknown.
    pub size: u64,
    /// Icon name or absolute icon path.
    pub icon: Option<String>,
    /// Main launcher.
    pub desktop: Option<PathBuf>,
    /// Other launchers that start the same software (e.g. copies made by install scripts).
    pub extra_launchers: Vec<PathBuf>,
    /// Native packages: the architecture (`amd64`, `x86_64`, `all`, `noarch`, `any`, ...).
    pub arch: String,
    /// Pacman: not from the configured repositories (usually built from the AUR).
    pub foreign: bool,
    /// Installed on purpose by the user rather than pulled in as a dependency.
    pub user_installed: bool,
    /// Why this can't be removed from Depot, if it can't.
    pub blocked: Option<String>,
    pub publisher: String,
    pub section: String,
    pub homepage: String,
    pub channel: String,
    pub revision: String,
    pub location: String,
    /// A launcher whose program is no longer on the computer.
    pub broken: bool,
    /// Flatpak: installed system-wide rather than for this user only.
    pub system: bool,
    /// Flatpak: the branch that starts by default when several are installed.
    pub current: bool,
    pub manual: Option<ManualPlan>,
    /// Developer tools: the files and folders that are theirs (programs, package folders), so
    /// launchers that start them are matched to them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<PathBuf>,
}

impl Package {
    pub fn new(kind: Kind, id: impl Into<String>) -> Self {
        let id = id.into();
        Package { kind, name: id.clone(), title: id.clone(), id, ..Default::default() }
    }

    /// Removing it takes the program away too, not only its launcher: always for an AppImage, and
    /// for a manual install when its own folder is known.
    pub fn removes_program(&self) -> bool {
        match self.kind {
            Kind::AppImage => true,
            Kind::Manual => self.manual.as_ref().is_some_and(|m| m.appdir.is_some()),
            Kind::Deb | Kind::Rpm | Kind::Pacman | Kind::Snap | Kind::Flatpak => false,
            Kind::Cargo | Kind::Npm | Kind::Pipx | Kind::Brew | Kind::Nix => false,
        }
    }

    /// Lowercased text matched by search.
    pub fn search_text(&self) -> String {
        format!("{} {} {}", self.title, self.name, self.summary).to_lowercase()
    }
}

#[cfg(test)]
mod tests {
    use super::Kind;

    #[test]
    fn all_lists_every_kind_once() {
        // a new kind must be added to ALL too: its badge would otherwise be missing from counts
        for (i, k) in Kind::ALL.iter().enumerate() {
            assert_eq!(*k as usize, i, "{k:?}");
        }
        assert_eq!(Kind::ALL.len(), Kind::Nix as usize + 1, "ALL ends with the last kind");
    }
}
