//! A small async client for the PackageKit system service, over D-Bus.
//! PackageKit installs, removes and updates distro packages (apt, dnf, zypper, pacman, ...)
//! with one polkit prompt and real progress reporting.

use adw::prelude::*;
use futures_channel::mpsc;
use futures_util::StreamExt;
use gtk::{gio, glib};

const NAME: &str = "org.freedesktop.PackageKit";
const TX_IFACE: &str = "org.freedesktop.PackageKit.Transaction";

/// Transaction flags and filters are bitfields of `1 << enum value`.
pub const ONLY_TRUSTED: u64 = 1 << 1;
pub const SIMULATE: u64 = 1 << 2;
pub const FILTER_NONE: u64 = 1 << 1;
pub const FILTER_INSTALLED: u64 = 1 << 2;

const EXIT_SUCCESS: u32 = 1;
const EXIT_CANCELLED: u32 = 3;
const ERROR_NOT_AUTHORIZED: u32 = 48;
const ERROR_TRANSACTION_CANCELLED: u32 = 17;
pub const ERROR_DEP_RESOLUTION_FAILED: u32 = 13;
pub const ERROR_CANNOT_REMOVE_SYSTEM_PACKAGE: u32 = 20;

/// Transaction `Status` values (PkStatusEnum) that change what the user is told.
pub const STATUS_WAITING_FOR_LOCK: u32 = 30;
pub const STATUS_WAITING_FOR_AUTH: u32 = 31;

#[derive(Clone, Debug)]
pub struct PkPackage {
    pub info: u32,
    pub id: String,
}

impl PkPackage {
    pub fn name(&self) -> &str {
        self.id.split(';').next().unwrap_or(&self.id)
    }
}

#[derive(Clone, Debug)]
pub struct PkError {
    pub code: u32,
    pub details: String,
    /// The user dismissed the password prompt or cancelled.
    pub cancelled: bool,
}

impl PkError {
    fn plain(details: impl Into<String>) -> Self {
        PkError { code: 0, details: details.into(), cancelled: false }
    }
}

impl std::fmt::Display for PkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.details)
    }
}

enum Event {
    Package(PkPackage),
    Error(u32, String),
    Finished(u32),
    Progress(u32),
    Status(u32),
    AllowCancel(bool),
}

/// What a running transaction reports.
pub enum Report<'a> {
    /// How far along it is, `None` while PackageKit doesn't know.
    Fraction(Option<f64>),
    /// A package it's working on.
    Package(&'a PkPackage),
    /// A `Status` value, see the `STATUS_*` constants.
    Status(u32),
    /// Whether pressing Cancel can still stop it.
    CanCancel(bool),
}

/// Whether PackageKit is installed (it's started on demand, so this doesn't need it running).
pub async fn available() -> bool {
    let Ok(conn) = gio::bus_get_future(gio::BusType::System).await else { return false };
    let reply = conn
        .call_future(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "ListActivatableNames",
            None,
            Some(glib::VariantTy::new("(as)").expect("type")),
            gio::DBusCallFlags::NONE,
            3000,
        )
        .await;
    reply.ok().and_then(|v| v.child_value(0).get::<Vec<String>>()).is_some_and(|names| names.iter().any(|n| n == NAME))
}

fn locale() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        .unwrap_or_else(|| "C".into())
}

async fn transaction(interactive: bool) -> Result<gio::DBusProxy, PkError> {
    let err = |e: glib::Error| PkError::plain(format!("PackageKit isn't available: {}", e.message()));
    let flags = gio::DBusProxyFlags::DO_NOT_LOAD_PROPERTIES | gio::DBusProxyFlags::DO_NOT_CONNECT_SIGNALS;
    let main =
        gio::DBusProxy::for_bus_future(gio::BusType::System, flags, None, NAME, "/org/freedesktop/PackageKit", NAME)
            .await
            .map_err(err)?;
    let r = main.call_future("CreateTransaction", None, gio::DBusCallFlags::NONE, 10_000).await.map_err(err)?;
    let path =
        r.child_value(0).str().map(str::to_string).ok_or_else(|| PkError::plain("PackageKit sent a bad reply"))?;
    let tx =
        gio::DBusProxy::for_bus_future(gio::BusType::System, gio::DBusProxyFlags::NONE, None, NAME, &path, TX_IFACE)
            .await
            .map_err(err)?;
    let hints =
        vec![format!("interactive={interactive}"), format!("locale={}", locale()), "background=false".to_string()];
    tx.call_future("SetHints", Some(&(hints,).to_variant()), gio::DBusCallFlags::NONE, 10_000).await.map_err(err)?;
    Ok(tx)
}

/// Run one transaction method and collect the packages it reports. Cancelling `cancel` asks
/// PackageKit to stop, which it does whenever it last reported it can.
pub async fn run(
    interactive: bool,
    method: &str,
    args: glib::Variant,
    cancel: &gio::Cancellable,
    report: impl Fn(Report<'_>) + 'static,
) -> Result<Vec<PkPackage>, PkError> {
    let tx = transaction(interactive).await?;
    let stop = {
        let tx = tx.clone();
        cancel.connect_cancelled_local(move |_| {
            let tx = tx.clone();
            glib::spawn_future_local(async move {
                let _ = tx.call_future("Cancel", None, gio::DBusCallFlags::NONE, 10_000).await;
            });
        })
    };
    let (send, recv) = mpsc::unbounded::<Event>();
    let s = send.clone();
    tx.connect_g_signal(move |_, _, signal, params| {
        let ev = match signal {
            "Package" => {
                params.get::<(u32, String, String)>().map(|(info, id, _summary)| Event::Package(PkPackage { info, id }))
            }
            "ErrorCode" => params.get::<(u32, String)>().map(|(c, d)| Event::Error(c, d)),
            "Finished" => params.get::<(u32, u32)>().map(|(exit, _)| Event::Finished(exit)),
            // the transaction object went away without finishing
            "Destroy" => Some(Event::Finished(0)),
            _ => None,
        };
        if let Some(ev) = ev {
            let _ = s.unbounded_send(ev);
        }
    });
    tx.connect_g_properties_changed(move |_, changed, _| {
        let d = glib::VariantDict::new(Some(changed));
        if let Ok(Some(p)) = d.lookup::<u32>("Percentage") {
            let _ = send.unbounded_send(Event::Progress(p));
        }
        if let Ok(Some(s)) = d.lookup::<u32>("Status") {
            let _ = send.unbounded_send(Event::Status(s));
        }
        if let Ok(Some(c)) = d.lookup::<bool>("AllowCancel") {
            let _ = send.unbounded_send(Event::AllowCancel(c));
        }
    });
    let r = events(&tx, method, args, recv, report).await;
    if let Some(id) = stop {
        cancel.disconnect_cancelled(id);
    }
    r
}

async fn events(
    tx: &gio::DBusProxy,
    method: &str,
    args: glib::Variant,
    mut recv: mpsc::UnboundedReceiver<Event>,
    report: impl Fn(Report<'_>),
) -> Result<Vec<PkPackage>, PkError> {
    let call = tx.call_future(method, Some(&args), gio::DBusCallFlags::NONE, -1).await;
    if let Err(e) = call {
        let refused = e.message().contains("RefusedByPolicy") || e.message().contains("NotAuthorized");
        return Err(PkError { code: 0, details: e.message().to_string(), cancelled: refused });
    }
    let mut packages = Vec::new();
    let mut error: Option<(u32, String)> = None;
    while let Some(ev) = recv.next().await {
        match ev {
            Event::Package(p) => {
                report(Report::Package(&p));
                packages.push(p);
            }
            Event::Progress(n) => report(Report::Fraction(crate::system::fraction(n))),
            Event::Status(s) => report(Report::Status(s)),
            Event::AllowCancel(c) => report(Report::CanCancel(c)),
            Event::Error(c, d) => error = Some((c, d)),
            Event::Finished(EXIT_SUCCESS) => return Ok(packages),
            Event::Finished(exit) => {
                let (code, details) = error.unwrap_or((0, "The package manager stopped without saying why.".into()));
                let cancelled =
                    exit == EXIT_CANCELLED || matches!(code, ERROR_NOT_AUTHORIZED | ERROR_TRANSACTION_CANCELLED);
                return Err(PkError { code, details, cancelled });
            }
        }
    }
    Err(PkError::plain("Lost contact with PackageKit."))
}

/// A quick query nobody watches or cancels.
async fn query(method: &str, args: glib::Variant) -> Result<Vec<PkPackage>, PkError> {
    run(false, method, args, &gio::Cancellable::new(), |_| {}).await
}

/// Refresh the package lists from the internet. Allowed without a password for local users.
pub async fn refresh_cache() -> Result<(), PkError> {
    query("RefreshCache", (false,).to_variant()).await.map(|_| ())
}

pub async fn get_updates() -> Result<Vec<PkPackage>, PkError> {
    query("GetUpdates", (FILTER_NONE,).to_variant()).await
}

/// PackageKit ids of installed packages, by name.
pub async fn resolve_installed(names: &[String]) -> Result<Vec<PkPackage>, PkError> {
    query("Resolve", (FILTER_INSTALLED, names.to_vec()).to_variant()).await
}

/// Remove packages along with anything that depends on them, and with `autoremove`, dependencies
/// nobody needs any more.
/// With `simulate`, nothing changes and the result lists everything that would be removed.
pub async fn remove(
    ids: &[String],
    simulate: bool,
    autoremove: bool,
    cancel: &gio::Cancellable,
    report: impl Fn(Report<'_>) + 'static,
) -> Result<Vec<PkPackage>, PkError> {
    let flags = if simulate { ONLY_TRUSTED | SIMULATE } else { ONLY_TRUSTED };
    run(!simulate, "RemovePackages", (flags, ids.to_vec(), true, autoremove).to_variant(), cancel, report).await
}

/// A dry run of [`remove`]: everything that would be removed, changing nothing.
pub async fn simulate_remove(ids: &[String], autoremove: bool) -> Result<Vec<PkPackage>, PkError> {
    remove(ids, true, autoremove, &gio::Cancellable::new(), |_| {}).await
}

/// Update packages to the versions reported by [`get_updates`]. With `simulate`, nothing changes and
/// the result lists every package the update would touch.
pub async fn update(
    ids: &[String],
    simulate: bool,
    cancel: &gio::Cancellable,
    report: impl Fn(Report<'_>) + 'static,
) -> Result<Vec<PkPackage>, PkError> {
    let flags = if simulate { ONLY_TRUSTED | SIMULATE } else { ONLY_TRUSTED };
    run(!simulate, "UpdatePackages", (flags, ids.to_vec()).to_variant(), cancel, report).await
}

/// A dry run of [`update`]: every package the update would touch, changing nothing.
pub async fn simulate_update(ids: &[String]) -> Result<Vec<PkPackage>, PkError> {
    update(ids, true, &gio::Cancellable::new(), |_| {}).await
}

impl PkError {
    /// This error as an [`Outcome`](crate::system::Outcome) the user can read.
    pub fn outcome(self) -> crate::system::Outcome {
        if self.cancelled {
            return crate::system::Outcome::Cancelled;
        }
        let summary = crate::system::plain(&self.details, (self.code != 0).then_some(self.code));
        crate::system::Outcome::Failed { summary, details: self.details }
    }
}
