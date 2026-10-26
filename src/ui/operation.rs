//! The sheet shown while Depot changes something. It always says what's happening right now
//! (checking, waiting for your password, waiting for another installer, working) and ends by
//! saying exactly how it went, so nothing finishes silently.

use super::widgets::{label, wrapped};
use crate::system::Outcome;
use adw::prelude::*;
use futures_channel::oneshot;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

/// Where one item of a batch is. Items start out waiting.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ItemState {
    Working,
    Done,
    Failed,
    /// Not attempted, because an earlier step was cancelled.
    Skipped,
}

/// How the whole operation went.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    Success,
    /// Some items worked and some didn't.
    Partial,
    Cancelled,
    Failed,
}

/// The result page's text.
pub struct Result {
    pub ending: Ending,
    pub heading: String,
    pub body: String,
    /// What the package manager said, behind "Show details".
    pub details: String,
    /// Offer "Try again".
    pub retry: bool,
}

/// What the user picked on the result page.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Done,
    Retry,
}

struct Row {
    state: gtk::Stack,
    note: gtk::Label,
}

pub struct Operation {
    dialog: adw::Dialog,
    pages: gtk::Stack,
    phase: gtk::Stack,
    status: gtk::Label,
    hint: gtk::Label,
    authorized: gtk::Revealer,
    bar: gtk::ProgressBar,
    rows: Vec<Row>,
    cancel: gtk::Button,
    cancellable: RefCell<Option<gio::Cancellable>>,
    pulsing: Rc<Cell<bool>>,
    /// Smooths progress bar jumps.
    bar_anim: adw::TimedAnimation,
    result: ResultPage,
    choice: RefCell<Option<oneshot::Sender<Choice>>>,
    hovered: gtk::EventControllerMotion,
    closed: Rc<Cell<bool>>,
    /// Something was already changed, so "Nothing has changed yet" would be wrong.
    changed: Cell<bool>,
}

struct ResultPage {
    icon: gtk::Image,
    heading: gtk::Label,
    body: gtk::Label,
    details: gtk::Expander,
    details_text: gtk::Label,
    copy: gtk::Button,
    retry: gtk::Button,
    done: gtk::Button,
}

fn centered(l: gtk::Label) -> gtk::Label {
    l.set_justify(gtk::Justification::Center);
    l.set_xalign(0.5);
    l
}

fn state_icon(name: &str, class: &str) -> gtk::Image {
    let i = gtk::Image::from_icon_name(name);
    if !class.is_empty() {
        i.add_css_class(class);
    }
    i
}

impl Operation {
    /// Open the sheet. `items` are shown as a checklist when there's more than one.
    pub fn show(parent: &impl IsA<gtk::Widget>, title: &str, items: &[String]) -> Rc<Operation> {
        let dialog = adw::Dialog::builder().content_width(420).can_close(false).title(title).build();
        let pages = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(200)
            .vhomogeneous(false)
            .interpolate_size(true)
            .build();

        // ---- working page
        let bx = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(28)
            .margin_bottom(24)
            .margin_start(24)
            .margin_end(24)
            .build();
        let phase = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(180)
            .halign(gtk::Align::Center)
            .build();
        let spinner = gtk::Spinner::builder().spinning(true).width_request(48).height_request(48).build();
        phase.add_named(&spinner, Some("busy"));
        let lock = gtk::Image::builder().icon_name("dialog-password-symbolic").pixel_size(48).build();
        lock.add_css_class("op-pulse");
        phase.add_named(&lock, Some("password"));
        let wait = gtk::Image::builder().icon_name("preferences-system-time-symbolic").pixel_size(48).build();
        wait.add_css_class("op-pulse");
        phase.add_named(&wait, Some("lock"));
        bx.append(&phase);
        let heading = centered(wrapped(title, &["title-3"]));
        bx.append(&heading);
        let status = centered(wrapped("Getting ready…", &["dim-label"]));
        bx.append(&status);

        let ok = gtk::Box::builder().spacing(6).halign(gtk::Align::Center).build();
        ok.append(&state_icon("emblem-ok-symbolic", "success"));
        ok.append(&label("Password accepted", &["success", "caption-heading"]));
        let authorized = gtk::Revealer::builder()
            .child(&ok)
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .transition_duration(200)
            .build();
        bx.append(&authorized);

        let bar = gtk::ProgressBar::new();
        bx.append(&bar);
        let hint = centered(wrapped("", &["caption", "dim-label"]));
        hint.set_visible(false);
        bx.append(&hint);

        let mut rows = Vec::new();
        if items.len() > 1 {
            let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
            list.add_css_class("boxed-list");
            for it in items {
                let row = adw::ActionRow::builder().title(glib::markup_escape_text(it)).build();
                let state = gtk::Stack::builder()
                    .transition_type(gtk::StackTransitionType::Crossfade)
                    .transition_duration(150)
                    .build();
                state.add_named(&state_icon("content-loading-symbolic", "dim-label"), Some("waiting"));
                state.add_named(&gtk::Spinner::builder().spinning(true).build(), Some("working"));
                let done = state_icon("emblem-ok-symbolic", "success");
                done.add_css_class("op-pop");
                state.add_named(&done, Some("done"));
                state.add_named(&state_icon("dialog-error-symbolic", "error"), Some("failed"));
                state.add_named(&state_icon("action-unavailable-symbolic", "dim-label"), Some("skipped"));
                row.add_prefix(&state);
                let note = label("", &["caption", "dim-label"]);
                row.add_suffix(&note);
                list.append(&row);
                rows.push(Row { state, note });
            }
            let sc = gtk::ScrolledWindow::builder()
                .child(&list)
                .max_content_height(220)
                .propagate_natural_height(true)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .margin_top(6)
                .build();
            bx.append(&sc);
        }

        let cancel = gtk::Button::builder().label("Cancel").halign(gtk::Align::Center).visible(false).build();
        cancel.add_css_class("pill");
        bx.append(&cancel);
        pages.add_named(&bx, Some("work"));

        // ---- result page
        let rb = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(28)
            .margin_bottom(24)
            .margin_start(24)
            .margin_end(24)
            .build();
        let icon = gtk::Image::builder().pixel_size(64).halign(gtk::Align::Center).build();
        rb.append(&icon);
        let r_heading = centered(wrapped("", &["title-2"]));
        rb.append(&r_heading);
        let body = centered(wrapped("", &[]));
        body.set_selectable(true);
        rb.append(&body);
        let details_text = gtk::Label::builder().xalign(0.0).selectable(true).wrap(true).build();
        details_text.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        details_text.add_css_class("monospace");
        details_text.add_css_class("caption");
        let copy = gtk::Button::builder().label("Copy details").halign(gtk::Align::Start).build();
        copy.add_css_class("flat");
        let dbox = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).margin_top(6).build();
        dbox.append(
            &gtk::ScrolledWindow::builder()
                .child(&details_text)
                .max_content_height(180)
                .propagate_natural_height(true)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .build(),
        );
        dbox.append(&copy);
        let details = gtk::Expander::builder().label("Show details").child(&dbox).build();
        rb.append(&details);
        let buttons = gtk::Box::builder().spacing(10).halign(gtk::Align::Center).margin_top(6).build();
        let retry = gtk::Button::with_label("Try again");
        retry.add_css_class("pill");
        let done = gtk::Button::with_label("Done");
        done.add_css_class("pill");
        done.add_css_class("suggested-action");
        buttons.append(&retry);
        buttons.append(&done);
        rb.append(&buttons);
        pages.add_named(&rb, Some("result"));

        let tv = adw::ToolbarView::new();
        tv.set_content(Some(&pages));
        dialog.set_child(Some(&tv));
        let hovered = gtk::EventControllerMotion::new();
        tv.add_controller(hovered.clone());
        dialog.present(Some(parent));

        let pulsing = Rc::new(Cell::new(true));
        let closed = Rc::new(Cell::new(false));
        {
            let (b, p, c) = (bar.clone(), pulsing.clone(), closed.clone());
            glib::timeout_add_local(Duration::from_millis(120), move || {
                if c.get() {
                    return glib::ControlFlow::Break;
                }
                if p.get() {
                    b.pulse();
                }
                glib::ControlFlow::Continue
            });
        }
        let bar_anim = {
            let b = bar.clone();
            adw::TimedAnimation::builder()
                .widget(&bar)
                .value_from(0.0)
                .value_to(0.0)
                .duration(250)
                .easing(adw::Easing::EaseOutCubic)
                .target(&adw::CallbackAnimationTarget::new(move |v| b.set_fraction(v)))
                .build()
        };

        let op = Rc::new(Operation {
            dialog,
            pages,
            phase,
            status,
            hint,
            authorized,
            bar,
            rows,
            cancel,
            cancellable: RefCell::new(None),
            pulsing,
            bar_anim,
            result: ResultPage { icon, heading: r_heading, body, details, details_text, copy, retry, done },
            choice: RefCell::new(None),
            hovered,
            closed,
            changed: Cell::new(false),
        });
        {
            let w = Rc::downgrade(&op);
            op.cancel.connect_clicked(move |b| {
                if let Some(op) = w.upgrade()
                    && let Some(c) = op.cancellable.borrow().as_ref()
                {
                    b.set_sensitive(false);
                    b.set_label("Cancelling…");
                    c.cancel();
                }
            });
        }
        {
            let w = Rc::downgrade(&op);
            op.result.retry.connect_clicked(move |_| {
                if let Some(op) = w.upgrade() {
                    op.choose(Choice::Retry);
                }
            });
        }
        {
            let w = Rc::downgrade(&op);
            op.result.done.connect_clicked(move |_| {
                if let Some(op) = w.upgrade() {
                    op.choose(Choice::Done);
                }
            });
        }
        {
            let w = Rc::downgrade(&op);
            op.dialog.connect_closed(move |_| {
                if let Some(op) = w.upgrade() {
                    op.closed.set(true);
                    if let Some(tx) = op.choice.borrow_mut().take() {
                        let _ = tx.send(Choice::Done);
                    }
                }
            });
        }
        {
            let text = op.result.details_text.clone();
            op.result.copy.connect_clicked(move |b| {
                b.clipboard().set_text(&text.text());
                b.set_label("Copied");
            });
        }
        op
    }

    fn choose(&self, c: Choice) {
        if let Some(tx) = self.choice.borrow_mut().take() {
            let _ = tx.send(c);
        }
        if c == Choice::Done {
            self.close();
        }
    }

    /// Close without a result page (the user backed out before anything happened).
    pub fn close(&self) {
        self.closed.set(true);
        self.dialog.set_can_close(true);
        self.dialog.force_close();
    }

    fn show_phase(&self, phase: &str) {
        self.pages.set_visible_child_name("work");
        self.phase.set_visible_child_name(phase);
    }

    /// Tell screen readers about a change they wouldn't otherwise hear about.
    fn announce(&self, text: &str) {
        self.dialog
            .upcast_ref::<gtk::Widget>()
            .announce(text.trim_end_matches(['.', ' ']), gtk::AccessibleAnnouncementPriority::Medium);
    }

    /// A small line under the progress bar; empty hides it.
    pub fn set_hint(&self, text: &str) {
        self.hint.set_text(text);
        self.hint.set_visible(!text.is_empty());
    }

    /// Working out what will happen, before anything changes.
    pub fn checking(&self, text: &str) {
        self.show_phase("busy");
        self.status.set_text(text);
        self.set_hint(if self.changed.get() { "" } else { "Nothing has changed yet." });
        self.pulsing.set(true);
    }

    /// The system's password window is open.
    pub fn waiting_for_password(&self) {
        self.show_phase("password");
        self.status.set_text("Waiting for your password");
        self.announce("Waiting for your password. Type it in the window that just opened.");
        self.set_hint(
            "Type your password in the window that just opened. That's your computer checking it's really you, not Depot asking.",
        );
        self.pulsing.set(true);
    }

    /// The password worked; the work itself starts now.
    pub fn authorized(&self) {
        self.show_phase("busy");
        self.authorized.set_reveal_child(true);
        self.set_hint("");
    }

    /// Another program holds the package database.
    pub fn waiting_for_lock(&self) {
        self.show_phase("lock");
        self.status.set_text("Waiting for another app to finish");
        self.announce("Waiting for another app to finish");
        self.set_hint("Another app is installing or updating software. Depot carries on as soon as it's done.");
        self.pulsing.set(true);
    }

    /// What's happening now (empty text keeps the old line) and how far along (`None` = unknown).
    pub fn progress(&self, text: &str, fraction: Option<f64>) {
        self.working_on(text);
        match fraction {
            Some(f) => {
                self.pulsing.set(false);
                self.bar_anim.pause();
                self.bar_anim.set_value_from(self.bar.fraction());
                self.bar_anim.set_value_to(f.clamp(0.0, 1.0));
                self.bar_anim.play();
            }
            None => self.pulsing.set(true),
        }
    }

    /// Mark item `i` of the checklist, with a short note ("Removed", "Couldn't remove").
    pub fn item(&self, i: usize, state: ItemState, note: &str) {
        if state == ItemState::Done {
            self.changed.set(true);
        }
        let Some(r) = self.rows.get(i) else { return };
        r.state.set_visible_child_name(match state {
            ItemState::Working => "working",
            ItemState::Done => "done",
            ItemState::Failed => "failed",
            ItemState::Skipped => "skipped",
        });
        r.note.set_text(note);
    }

    /// Offer Cancel while `c` can still stop the work; `None` hides it. Once Cancel was pressed
    /// it keeps saying "Cancelling…", however often the package manager says it can be cancelled.
    pub fn cancellable(&self, c: Option<&gio::Cancellable>) {
        *self.cancellable.borrow_mut() = c.cloned();
        let asked = c.is_some_and(|c| c.is_cancelled());
        self.cancel.set_visible(c.is_some());
        self.cancel.set_sensitive(!asked);
        self.cancel.set_label(if asked { "Cancelling…" } else { "Cancel" });
    }

    /// What's being worked on right now (empty text keeps the old line), without touching the
    /// progress bar.
    pub fn working_on(&self, text: &str) {
        if self.phase.visible_child_name().as_deref() == Some("lock") {
            self.set_hint("");
        }
        self.show_phase("busy");
        if !text.is_empty() {
            self.status.set_text(text);
        }
    }

    /// Show how it went and wait for the user's choice. A plain success closes by itself after a
    /// moment, unless the pointer is over the sheet.
    pub async fn finish(self: &Rc<Self>, r: Result) -> Choice {
        if self.closed.get() {
            return Choice::Done;
        }
        self.pulsing.set(false);
        self.cancellable(None);
        let p = &self.result;
        let (icon, class) = match r.ending {
            Ending::Success => ("emblem-ok-symbolic", "success"),
            Ending::Partial => ("dialog-warning-symbolic", "warning"),
            Ending::Cancelled => ("action-unavailable-symbolic", "dim-label"),
            Ending::Failed => ("dialog-error-symbolic", "error"),
        };
        p.icon.set_icon_name(Some(icon));
        for c in ["success", "warning", "dim-label", "error"] {
            p.icon.remove_css_class(c);
        }
        p.icon.add_css_class(class);
        // replaying the class restarts the pop-in animation
        p.icon.remove_css_class("op-pop");
        p.icon.add_css_class("op-pop");
        p.heading.set_text(&r.heading);
        self.announce(&format!("{}. {}", r.heading, r.body));
        p.body.set_text(&r.body);
        p.body.set_visible(!r.body.is_empty());
        p.details_text.set_text(r.details.trim());
        p.details.set_visible(!r.details.trim().is_empty());
        p.details.set_expanded(false);
        p.copy.set_label("Copy details");
        p.retry.set_visible(r.retry);
        p.done.set_label(if r.ending == Ending::Cancelled { "Close" } else { "Done" });
        self.pages.set_visible_child_name("result");
        self.dialog.set_can_close(true);
        if r.retry {
            p.retry.add_css_class("suggested-action");
            p.done.remove_css_class("suggested-action");
            p.retry.grab_focus();
        } else {
            p.done.grab_focus();
        }

        let (tx, rx) = oneshot::channel();
        *self.choice.borrow_mut() = Some(tx);
        if r.ending == Ending::Success {
            let w = Rc::downgrade(self);
            glib::timeout_add_local_once(Duration::from_millis(2500), move || {
                if let Some(op) = w.upgrade()
                    && !op.closed.get()
                    && !op.hovered.contains_pointer()
                {
                    op.choose(Choice::Done);
                }
            });
        }
        rx.await.unwrap_or(Choice::Done)
    }

    /// Back to the working page for another go in the same sheet, after "Try again".
    pub fn reset(&self) {
        self.dialog.set_can_close(false);
        self.authorized.set_reveal_child(false);
        self.bar.set_fraction(0.0);
        self.result.retry.remove_css_class("suggested-action");
        self.result.done.add_css_class("suggested-action");
        self.checking("Getting ready…");
    }
}

/// Turns what a PackageKit transaction reports into the sheet's states. `working` names the step
/// for one package ("Removing firefox…").
pub fn pk_reporter(
    op: &Rc<Operation>,
    cancel: &gio::Cancellable,
    working: impl Fn(&str) -> String + 'static,
) -> impl Fn(crate::packagekit::Report<'_>) + 'static {
    use crate::packagekit::{Report, STATUS_WAITING_FOR_AUTH, STATUS_WAITING_FOR_LOCK};
    let (op, cancel) = (op.clone(), cancel.clone());
    let asked = Cell::new(false);
    move |r| match r {
        Report::Status(STATUS_WAITING_FOR_AUTH) => {
            asked.set(true);
            op.waiting_for_password();
        }
        Report::Status(STATUS_WAITING_FOR_LOCK) => op.waiting_for_lock(),
        Report::Status(_) if asked.replace(false) => op.authorized(),
        Report::Status(_) => {}
        Report::Package(p) => {
            if asked.replace(false) {
                op.authorized();
            }
            // PackageKit sends these between percentages: the bar keeps its place
            op.working_on(&working(p.name()));
        }
        Report::Fraction(f) => {
            if asked.replace(false) {
                op.authorized();
            }
            op.progress("", f);
        }
        Report::CanCancel(yes) => op.cancellable(yes.then_some(&cancel)),
    }
}

/// One piece of work in the sheet: the checklist items it covers, and what does it.
pub struct Step<W> {
    pub items: Vec<usize>,
    pub work: W,
}

/// How a run of steps went.
pub struct Ran<W> {
    /// Checklist items whose step worked.
    pub done: Vec<bool>,
    /// The steps that failed or were cancelled: their items and how they ended.
    pub failures: Vec<(Vec<usize>, Outcome)>,
    /// Steps that didn't work or never started, for "Try again".
    pub left: Vec<Step<W>>,
}

/// Run `steps` one after another, marking their items in the checklist of `n` items. Once one is
/// cancelled the rest aren't started. `done_note` is what a finished item says ("Removed").
pub async fn run_steps<W>(
    op: &Rc<Operation>,
    n: usize,
    steps: Vec<Step<W>>,
    done_note: impl Fn(&W) -> &'static str,
    mut run: impl AsyncFnMut(&Step<W>) -> Outcome,
) -> Ran<W> {
    let mut ran = Ran { done: vec![false; n], failures: Vec::new(), left: Vec::new() };
    let mut stopped = false;
    for step in steps {
        if stopped {
            step.items.iter().for_each(|&i| op.item(i, ItemState::Skipped, "Not started"));
            ran.left.push(step);
            continue;
        }
        step.items.iter().for_each(|&i| op.item(i, ItemState::Working, ""));
        // a hint from the step before, or from the check beforehand, is about something else
        op.set_hint("");
        let o = run(&step).await;
        let (state, note) = match &o {
            Outcome::Done => (ItemState::Done, done_note(&step.work)),
            Outcome::Cancelled => (ItemState::Skipped, "Cancelled"),
            Outcome::Failed { .. } => (ItemState::Failed, "Failed"),
        };
        for &i in &step.items {
            ran.done[i] = state == ItemState::Done;
            op.item(i, state, note);
        }
        if !matches!(o, Outcome::Done) {
            stopped |= matches!(o, Outcome::Cancelled);
            ran.failures.push((step.items.clone(), o));
            ran.left.push(step);
        }
    }
    ran
}

/// Run a package-manager command (as root when `root` is set) inside the sheet: it shows the
/// password step, then `working`, then the command's own short progress lines.
pub async fn command(op: &Rc<Operation>, cmd: &[String], root: bool, working: &str) -> Outcome {
    use crate::system::{self, Step};
    let cancel = gio::Cancellable::new();
    // Flatpak asks for the password itself for what's installed for everyone
    let may_ask = !root && cmd.first().is_some_and(|c| c == "flatpak") && cmd.iter().any(|a| a == "--system");
    if root {
        op.waiting_for_password();
    } else {
        op.progress(working, None);
    }
    if may_ask {
        op.set_hint("You may be asked for your password.");
    }
    // a root command can only be stopped before the password is entered
    op.cancellable(Some(&cancel));
    let r = system::run(cmd, root, &cancel, |step| match step {
        Step::Authorized => {
            op.authorized();
            op.cancellable(None);
            op.progress(working, None);
        }
        Step::Line(l) => {
            // progress bars redraw with \r; the last frame is the current one
            let l = l.rsplit('\r').next().unwrap_or(l).trim();
            // apt waiting for Software or automatic updates to let go of the package database
            if l.starts_with("Waiting for cache lock") {
                op.waiting_for_lock();
            } else if !l.is_empty() && l.chars().count() <= 90 {
                op.progress(l, None);
            }
        }
    })
    .await;
    op.cancellable(None);
    if may_ask {
        op.set_hint("");
    }
    r
}
