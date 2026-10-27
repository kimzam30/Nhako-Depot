//! A screenshot tour for checking layouts, in debug builds only: `DEPOT_TOUR=<folder> depot`
//! opens a phone-sized window, visits every screen, saves each as a PNG and writes how wide the
//! window had to be (`report.txt`). Anything wider than 360 px doesn't fit a phone.

use super::operation::{self, Ending, ItemState, Operation};
use super::{State, detail, item, remove, select};
use adw::prelude::*;
use depot_core::{Kind, Package, Role};
use gtk::glib;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

pub const WIDTH: i32 = 360;
pub const HEIGHT: i32 = 780;

type Visit = Box<dyn Fn(&Rc<State>)>;

thread_local! {
    /// The progress sheet shown by the tour, so the next screen can finish it.
    static SHEET: std::cell::RefCell<Option<Rc<Operation>>> = const { std::cell::RefCell::new(None) };
}

fn shown(st: &State) -> Vec<Package> {
    (0..st.filtered.n_items()).filter_map(|i| st.filtered.item(i)).map(|o| item(&o).p().clone()).collect()
}

fn installed(st: &State, f: impl Fn(&Package) -> bool) -> Option<Package> {
    st.packages.borrow().iter().find(|p| f(p)).cloned()
}

fn go_to(st: &State, row: usize) {
    if let Some(list) = st.rows[row].row.parent().and_downcast::<gtk::ListBox>() {
        list.unselect_all();
        list.select_row(Some(&st.rows[row].row));
    }
}

fn sidebar(st: &State, show: bool) {
    if let Some(split) = st.main_page.child().and_downcast::<adw::OverlaySplitView>() {
        split.set_show_sidebar(show);
    }
}

fn visits() -> Vec<(&'static str, u64, Visit)> {
    vec![
        ("grid", 2000, Box::new(|st| go_to(st, 0))),
        ("sidebar", 800, Box::new(|st| sidebar(st, true))),
        (
            "sidebar-advanced",
            800,
            Box::new(|st| {
                sidebar(st, true);
                if let Some(r) = st.rows.iter().find(|r| r.nav == super::Nav::Advanced) {
                    r.row.activate();
                }
            }),
        ),
        (
            "select",
            800,
            Box::new(|st| {
                sidebar(st, false);
                select::set_mode(st, true);
                for p in shown(st).iter().take(2) {
                    select::toggle(st, p);
                }
            }),
        ),
        (
            "detail",
            1000,
            Box::new(|st| {
                select::set_mode(st, false);
                // the copy with the most badges is the hardest to fit
                let all = shown(st);
                if let Some(p) = all.iter().find(|p| p.kind == Kind::Flatpak).or(all.first()) {
                    detail::show(st, p.clone(), Vec::new());
                }
            }),
        ),
        (
            "confirm",
            2000,
            Box::new(|st| {
                if let Some(p) = installed(st, |p| p.kind == Kind::Flatpak && p.role == Role::App) {
                    remove::start(st, vec![p], None);
                }
            }),
        ),
        (
            "confirm-several",
            5000,
            Box::new(|st| {
                let picks: Vec<Package> = [Kind::Deb, Kind::Flatpak, Kind::Manual]
                    .iter()
                    .filter_map(|&k| installed(st, |p| p.kind == k && p.role == Role::App && p.blocked.is_none()))
                    .collect();
                remove::start(st, picks, None);
            }),
        ),
        ("updates", 8000, Box::new(|st| go_to(st, 1))),
        (
            "progress",
            1500,
            Box::new(|st| {
                go_to(st, 0);
                let names: Vec<String> =
                    ["Firefox Web Browser", "GNU Image Manipulation Program", "Visual Studio Code"]
                        .map(String::from)
                        .to_vec();
                let op = Operation::show(&st.win, "Uninstalling 3 apps", &names);
                op.item(0, ItemState::Done, "Removed");
                op.item(1, ItemState::Working, "");
                op.progress("Removing GNU Image Manipulation Program…", Some(0.4));
                SHEET.set(Some(op));
            }),
        ),
        (
            "progress-result",
            1500,
            Box::new(|_| {
                let Some(op) = SHEET.with_borrow(Clone::clone) else { return };
                op.item(1, ItemState::Failed, "Failed");
                op.item(2, ItemState::Skipped, "Not started");
                let r = operation::Result {
                    ending: Ending::Partial,
                    heading: "1 of 3 apps were removed".into(),
                    body: "GNU Image Manipulation Program, Visual Studio Code are still installed. Another app is \
                           installing or updating software right now. Wait until it's finished, then try again."
                        .into(),
                    details: "E: Could not get lock /var/lib/dpkg/lock-frontend".into(),
                    retry: true,
                };
                glib::spawn_future_local(async move {
                    op.finish(r).await;
                });
            }),
        ),
        (
            "runtimes",
            1500,
            Box::new(|st| {
                if let Some(op) = SHEET.take() {
                    op.close();
                }
                let all = st.packages.borrow().clone();
                let picked: Vec<Package> = st.unused_runtimes.borrow().iter().map(|&i| all[i].clone()).collect();
                remove::clean_up_runtimes(st, picked);
            }),
        ),
        ("about", 1500, Box::new(|st| gtk::prelude::ActionGroupExt::activate_action(&st.win, "about", None))),
    ]
}

/// The chain of widest children from `w` down, with each step's siblings: what makes it wide.
fn widest(w: &gtk::Widget, depth: usize, out: &mut Vec<String>) {
    let min = |w: &gtk::Widget| w.measure(gtk::Orientation::Horizontal, -1).0;
    let mut kids = Vec::new();
    let mut c = w.first_child();
    while let Some(k) = c {
        // a stack is as wide as its widest page, shown or not
        if k.is_visible() || w.is::<gtk::Stack>() {
            kids.push(k.clone());
        }
        c = k.next_sibling();
    }
    if depth > 40 || kids.is_empty() {
        return;
    }
    let row: Vec<String> = kids
        .iter()
        .map(|k| {
            let label = k.downcast_ref::<gtk::Label>().map(|l| format!(" {:?}", l.text())).unwrap_or_default();
            format!("{}{label}={}", k.css_name(), min(k))
        })
        .collect();
    if kids.len() > 1 {
        out.push(format!("{}{} [{}] -> {}", "  ".repeat(depth.min(20)), w.css_name(), min(w), row.join(", ")));
    }
    if let Some(k) = kids.iter().max_by_key(|k| min(k)) {
        widest(k, depth + 1, out);
    }
}

/// Wait until the window has drawn itself again, so a picture shows the screen as it is now and
/// not the last frame drawn (with no browser watching, Broadway draws only when asked).
async fn fresh_frame(win: &adw::ApplicationWindow) {
    let Some(clock) = win.frame_clock() else { return };
    let (tx, rx) = futures_channel::oneshot::channel::<()>();
    let tx = std::cell::Cell::new(Some(tx));
    let id = clock.connect_after_paint(move |_| {
        if let Some(tx) = tx.take() {
            let _ = tx.send(());
        }
    });
    win.queue_draw();
    clock.request_phase(gtk::gdk::FrameClockPhase::PAINT);
    let timeout = glib::timeout_future(Duration::from_secs(2));
    futures_util::future::select(rx, Box::pin(timeout)).await;
    clock.disconnect(id);
}

/// The window drawn as it is now, saved as a PNG.
fn save(win: &adw::ApplicationWindow, path: &std::path::Path) -> Result<(), String> {
    let (w, h) = (win.width(), win.height());
    let paintable = gtk::WidgetPaintable::new(Some(win));
    let snap = gtk::Snapshot::new();
    paintable.snapshot(&snap, f64::from(w), f64::from(h));
    let node = snap.to_node().ok_or("nothing drawn")?;
    let renderer = win.renderer().ok_or("no renderer")?;
    let texture = renderer.render_texture(node, Some(&gtk::graphene::Rect::new(0.0, 0.0, w as f32, h as f32)));
    texture.save_to_png(path).map_err(|e| e.to_string())
}

/// Visit every screen once the installed list is read, then quit.
pub fn start(st: &Rc<State>, dir: PathBuf) {
    let _ = std::fs::create_dir_all(&dir);
    let st = st.clone();
    glib::spawn_future_local(async move {
        while st.loading.get() {
            glib::timeout_future(Duration::from_millis(200)).await;
        }
        let mut report = Vec::new();
        for (i, (name, wait, visit)) in visits().into_iter().enumerate() {
            if !name.starts_with("progress-result") {
                if let Some(d) = st.win.visible_dialog() {
                    d.force_close();
                }
                if !name.starts_with("detail") {
                    st.nav.pop_to_page(&st.main_page);
                }
            }
            visit(&st);
            glib::timeout_future(Duration::from_millis(wait)).await;
            // the window itself never asks for more than its 360 px minimum; what's inside does
            let min_of = |w: &gtk::Widget| w.measure(gtk::Orientation::Horizontal, -1).0;
            let content = min_of(st.toasts.upcast_ref());
            let dialog = st.win.visible_dialog().and_then(|d| d.child()).map_or(0, |c| min_of(&c));
            let min = content.max(dialog);
            fresh_frame(&st.win).await;
            let file = dir.join(format!("{:02}-{name}.png", i + 1));
            // the first frame after a change can come out empty; try again shortly
            let mut saved = save(&st.win, &file);
            for _ in 0..5 {
                if saved.is_ok() {
                    break;
                }
                glib::timeout_future(Duration::from_millis(300)).await;
                saved = save(&st.win, &file);
            }
            let saved = saved.err().map(|e| format!(" (not saved: {e})")).unwrap_or_default();
            let verdict = if min > WIDTH { "TOO WIDE" } else { "fits" };
            if i == 0 {
                let bp = st.win.current_breakpoint().and_then(|b| b.condition()).map(|c| c.to_str().to_string());
                let dpi = gtk::Settings::default().map(|s| s.gtk_xft_dpi()).unwrap_or(0);
                report.push(format!("breakpoint {bp:?}, xft-dpi {dpi}, window {}x{}", st.win.width(), st.win.height()));
            }
            if min > WIDTH {
                let mut why = Vec::new();
                match st.win.visible_dialog().and_then(|d| d.child()) {
                    Some(c) if dialog > WIDTH => widest(&c, 0, &mut why),
                    _ => widest(st.toasts.upcast_ref(), 0, &mut why),
                }
                let _ = std::fs::write(dir.join(format!("{:02}-{name}.why.txt", i + 1)), why.join("\n") + "\n");
            }
            report.push(format!("{name:<18} content {content:>4} px, dialog {dialog:>4} px: {verdict}{saved}"));
        }
        let _ = std::fs::write(dir.join("report.txt"), report.join("\n") + "\n");
        if let Some(app) = st.win.application() {
            app.quit();
        }
    });
}
