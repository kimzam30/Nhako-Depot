use serde::Serialize;
use std::path::PathBuf;

/// How a piece of software got onto the computer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Deb,
    Snap,
    Flatpak,
    Manual,
}

impl Kind {
    /// Short text for the badge on a card.
    pub fn badge(self) -> &'static str {
        match self {
            Kind::Deb => "Deb",
            Kind::Snap => "Snap",
            Kind::Flatpak => "Flatpak",
            Kind::Manual => "Manual",
        }
    }

    /// CSS suffix for `.badge-*`.
    pub fn css(self) -> &'static str {
        match self {
            Kind::Deb => "deb",
            Kind::Snap => "snap",
            Kind::Flatpak => "flatpak",
            Kind::Manual => "manual",
        }
    }

    /// Plain-language name of the source, for the details list.
    pub fn source(self) -> &'static str {
        match self {
            Kind::Deb => "Debian package (apt)",
            Kind::Snap => "Snap",
            Kind::Flatpak => "Flatpak",
            Kind::Manual => "Manual install (not deb, snap or flatpak)",
        }
    }

    /// One sentence for newcomers, shown as the badge tooltip.
    pub fn explain(self) -> &'static str {
        match self {
            Kind::Deb => "Installed from your system's software sources with apt.",
            Kind::Snap => "Installed from the Snap Store. Snaps update themselves.",
            Kind::Flatpak => "Installed with Flatpak, usually from Flathub. Runs in its own sandbox.",
            Kind::Manual => {
                "Installed by hand (downloaded archive, script or launcher), not through a package manager."
            }
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
}

impl Package {
    pub fn new(kind: Kind, id: impl Into<String>) -> Self {
        let id = id.into();
        Package { kind, name: id.clone(), title: id.clone(), id, ..Default::default() }
    }

    pub fn removable(&self) -> bool {
        self.blocked.is_none()
    }

    /// Lowercased text matched by search.
    pub fn search_text(&self) -> String {
        format!("{} {} {}", self.title, self.name, self.summary).to_lowercase()
    }
}
