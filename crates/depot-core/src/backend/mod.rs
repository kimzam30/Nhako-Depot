//! One module per way software can be installed.

pub mod dpkg;
pub mod flatpak;
pub mod manual;
pub mod pacman;
pub mod rpm;
pub mod snap;

use crate::model::{Kind, Package};

/// The id a native package manager uses for `name` built for `arch`: what Depot stores in
/// [`Package::id`] and passes to apt, dnf or pacman.
pub fn native_id(kind: Kind, name: &str, arch: &str) -> String {
    match kind {
        Kind::Deb => dpkg::pkg_id(name, arch),
        Kind::Rpm => rpm::pkg_id(name, arch),
        _ => name.to_string(),
    }
}

/// The native package kind of this system, judged from what's installed.
pub fn native_kind(inv: &[Package]) -> Kind {
    inv.iter().map(|p| p.kind).find(|k| k.is_native()).unwrap_or_default()
}
