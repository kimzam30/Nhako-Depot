//! Main window: category sidebar, searchable card grid, and navigation to the detail page.

mod about;
mod detail;
pub mod operation;
mod remove;
mod select;
mod updates;
mod widgets;

use adw::prelude::*;
use depot_core::{Env, Inventory, Kind, Package, Role};
use gtk::{gdk, gio, glib};
use std::cell::{Cell, Ref, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// One app of the shared inventory plus precomputed keys, stored in the list model. An app
/// installed several ways is one item: the best copy, with the other copies in `also`.
pub struct Item {
    all: Rc<Vec<Package>>,
    i: usize,
    also: Vec<usize>,
    search: String,
    sort: String,
}

impl Item {
    pub fn p(&self) -> &Package {
        &self.all[self.i]
    }

    /// The other copies of this app.
    pub fn others(&self) -> impl Iterator<Item = &Package> {
        self.also.iter().map(|&j| &self.all[j])
    }

    /// Whether any copy belongs in a category, so a Deb + Snap app shows under both.
    fn matches(&self, pred: fn(&Package) -> bool) -> bool {
        pred(self.p()) || self.others().any(pred)
    }
}

pub fn item(o: &glib::Object) -> Ref<'_, Item> {
    o.downcast_ref::<glib::BoxedAnyObject>().expect("list item").borrow::<Item>()
}

struct Cat {
    name: &'static str,
    icon: &'static str,
    advanced: bool,
    pred: fn(&Package) -> bool,
    /// Shown when the category is empty.
    empty: &'static str,
}

const CATS: &[Cat] = &[
    Cat {
        name: "All apps",
        icon: "view-grid-symbolic",
        advanced: false,
        pred: |p| p.role == Role::App && !p.broken,
        empty: "Depot didn't find any apps with a launcher in your app menu.",
    },
    Cat {
        name: "Deb apps",
        icon: "system-software-install-symbolic",
        advanced: false,
        pred: |p| p.role == Role::App && p.kind == Kind::Deb,
        empty: "Apps from your system's software sources show up here.",
    },
    Cat {
        name: "RPM apps",
        icon: "system-software-install-symbolic",
        advanced: false,
        pred: |p| p.role == Role::App && p.kind == Kind::Rpm,
        empty: "Apps from your system's software sources show up here.",
    },
    Cat {
        name: "Pacman apps",
        icon: "system-software-install-symbolic",
        advanced: false,
        pred: |p| p.role == Role::App && p.kind == Kind::Pacman,
        empty: "Apps from your system's software sources show up here.",
    },
    Cat {
        name: "Snap apps",
        icon: "package-x-generic-symbolic",
        advanced: false,
        pred: |p| p.role == Role::App && p.kind == Kind::Snap,
        empty: "Apps from the Snap Store show up here.",
    },
    Cat {
        name: "Flatpak apps",
        icon: "view-app-grid-symbolic",
        advanced: false,
        pred: |p| p.role == Role::App && p.kind == Kind::Flatpak,
        empty: "Apps installed with Flatpak, usually from Flathub, show up here.",
    },
    Cat {
        name: "AppImages",
        icon: "application-x-executable-symbolic",
        advanced: false,
        pred: |p| p.kind == Kind::AppImage,
        empty: "Apps you downloaded as a single AppImage file show up here.",
    },
    Cat {
        name: "Manual installs",
        icon: "folder-download-symbolic",
        advanced: false,
        pred: |p| p.kind == Kind::Manual && !p.broken,
        empty: "Apps you downloaded and installed by hand show up here.",
    },
    Cat {
        name: "Broken launchers",
        icon: "dialog-warning-symbolic",
        advanced: false,
        pred: |p| p.broken,
        empty: "Good news: every entry in your app menu still starts its app.",
    },
    Cat {
        name: "Tools you installed",
        icon: "utilities-terminal-symbolic",
        advanced: true,
        pred: |p| p.role == Role::Tool && p.user_installed,
        empty: "Command-line tools you installed yourself show up here.",
    },
    Cat {
        name: "All system packages",
        icon: "applications-engineering-symbolic",
        advanced: true,
        pred: |p| p.role == Role::Tool,
        empty: "Libraries and tools from your system's software sources show up here.",
    },
    Cat {
        name: "Runtimes & bases",
        icon: "applications-system-symbolic",
        advanced: true,
        pred: |p| p.role == Role::Runtime,
        empty: "Shared runtimes that Flatpaks and Snaps need show up here.",
    },
];

/// What a sidebar row opens.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Nav {
    Cat(usize),
    Updates,
    /// Shows or hides the advanced categories.
    Advanced,
}

struct SideRow {
    row: gtk::ListBoxRow,
    nav: Nav,
    count: gtk::Label,
    /// Slides the row in and out.
    reveal: gtk::Revealer,
    chevron: Option<gtk::Image>,
}

pub struct State {
    pub win: adw::ApplicationWindow,
    pub toasts: adw::ToastOverlay,
    pub nav: adw::NavigationView,
    pub main_page: adw::NavigationPage,
    store: gio::ListStore,
    filtered: gtk::FilterListModel,
    stack: gtk::Stack,
    empty: adw::StatusPage,
    no_results: adw::StatusPage,
    failed: adw::StatusPage,
    search: gtk::SearchEntry,
    loading: Cell<bool>,
    /// A scan is running; `rescan` asks for another once it's done instead of running two at once.
    scanning: Cell<bool>,
    rescan: Cell<bool>,
    rows: Vec<SideRow>,
    show_advanced: Cell<bool>,
    cat: Rc<Cell<usize>>,
    /// Everything installed, as last read; update checks match against it.
    packages: RefCell<Rc<Vec<Package>>>,
    updates: updates::View,
    /// Shown when this distro's own packages can't be read yet.
    unsupported: adw::Banner,
    /// "browse" (the grid) or "updates".
    view: gtk::Stack,
    browse_tools: Vec<gtk::Widget>,
    /// The grid's toolbar view, whose bottom bar is the selection bar.
    content: adw::ToolbarView,
    selection: Rc<select::Selection>,
    select: gtk::ToggleButton,
    select_all: gtk::Button,
    sel_label: gtk::Label,
    sel_remove: gtk::Button,
}

impl State {
    pub fn toast(&self, text: &str) {
        self.toasts.add_toast(adw::Toast::new(text));
    }

    fn on_updates(&self) -> bool {
        self.view.visible_child_name().as_deref() == Some("updates")
    }
}

/// Grey placeholder cards shown while the inventory is read.
fn skeleton() -> gtk::Widget {
    let fb = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .min_children_per_line(1)
        .max_children_per_line(8)
        .row_spacing(6)
        .column_spacing(6)
        .margin_start(12)
        .margin_end(12)
        .margin_top(12)
        .can_target(false)
        .valign(gtk::Align::Start)
        .build();
    for i in 0..12 {
        let b = gtk::Box::builder().width_request(210).height_request(190).build();
        b.add_css_class("card");
        b.add_css_class("skeleton");
        // a staggered start makes the shimmer travel across the grid
        b.add_css_class(&format!("skeleton-{}", i % 4));
        fb.append(&b);
    }
    gtk::ScrolledWindow::builder().child(&fb).hscrollbar_policy(gtk::PolicyType::Never).build().upcast()
}

pub fn activate(app: &adw::Application) {
    if let Some(w) = app.active_window() {
        w.present();
        return;
    }
    gtk::Window::set_default_icon_name(crate::APP_ID);
    let css = gtk::CssProvider::new();
    css.load_from_string(include_str!("../../data/style.css"));
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }

    let cat = Rc::new(Cell::new(0usize));
    let query = Rc::new(RefCell::new(String::new()));
    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    let filter = {
        let (cat, query) = (cat.clone(), query.clone());
        gtk::CustomFilter::new(move |o| {
            let it = item(o);
            it.matches(CATS[cat.get()].pred) && query.borrow().split_whitespace().all(|w| it.search.contains(w))
        })
    };
    let filtered = gtk::FilterListModel::new(Some(store.clone()), Some(filter.clone()));
    let sorted = gtk::SortListModel::new(Some(filtered.clone()), Some(sorter(0)));

    let win = adw::ApplicationWindow::builder()
        .application(app)
        .title("Depot")
        .default_width(1100)
        .default_height(720)
        // required by the narrow-window breakpoint; 360 px is GNOME's smallest phone width
        .width_request(360)
        .height_request(400)
        .build();
    let toasts = adw::ToastOverlay::new();
    let nav = adw::NavigationView::new();
    toasts.set_child(Some(&nav));
    win.set_content(Some(&toasts));

    // ---- sidebar
    let split = adw::OverlaySplitView::builder().sidebar_width_fraction(0.22).min_sidebar_width(210.0).build();
    let sb = adw::ToolbarView::new();
    sb.add_top_bar(
        &adw::HeaderBar::builder().title_widget(&adw::WindowTitle::new("Depot", "Everything installed")).build(),
    );
    let catlist = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Single).build();
    catlist.add_css_class("navigation-sidebar");
    let mut rows = Vec::new();
    let mut add_row = |name: &str, icon: &str, nav: Nav| {
        let row = gtk::ListBoxRow::new();
        let bx = gtk::Box::builder().spacing(10).margin_top(8).margin_bottom(8).margin_start(6).margin_end(6).build();
        let img = gtk::Image::from_icon_name(icon);
        bx.append(&img);
        bx.append(
            &gtk::Label::builder()
                .label(name)
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build(),
        );
        let count = widgets::label("", &["dim-label", "caption", "count"]);
        bx.append(&count);
        let reveal = gtk::Revealer::builder()
            .child(&bx)
            .reveal_child(true)
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .transition_duration(200)
            .build();
        row.set_child(Some(&reveal));
        let mut chevron = None;
        if nav == Nav::Advanced {
            row.set_selectable(false);
            row.set_tooltip_text(Some("Libraries, command-line tools and runtimes. Most people never need these."));
            img.add_css_class("chevron");
            chevron = Some(img);
        }
        catlist.append(&row);
        let r = SideRow { row, nav, count, reveal, chevron };
        hide_when_collapsed(&r);
        rows.push(r);
    };
    for (i, c) in CATS.iter().enumerate().filter(|(_, c)| !c.advanced) {
        add_row(c.name, c.icon, Nav::Cat(i));
        if i == 0 {
            add_row("Updates", "software-update-available-symbolic", Nav::Updates);
        }
    }
    add_row("Advanced", "pan-end-symbolic", Nav::Advanced);
    for (i, c) in CATS.iter().enumerate().filter(|(_, c)| c.advanced) {
        add_row(c.name, c.icon, Nav::Cat(i));
    }
    sb.set_content(Some(
        &gtk::ScrolledWindow::builder().child(&catlist).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build(),
    ));
    split.set_sidebar(Some(&sb));

    // ---- content
    let tv = adw::ToolbarView::new();
    let hb = adw::HeaderBar::new();
    let toggle = gtk::ToggleButton::builder()
        .icon_name("sidebar-show-symbolic")
        .active(true)
        .tooltip_text("Show categories")
        .build();
    toggle.bind_property("active", &split, "show-sidebar").bidirectional().build();
    hb.pack_start(&toggle);
    let search = gtk::SearchEntry::builder().placeholder_text("Search installed software").hexpand(true).build();
    hb.set_title_widget(Some(&adw::Clamp::builder().maximum_size(420).child(&search).build()));
    let menu = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .tooltip_text("Main menu")
        .menu_model(&about::menu())
        .primary(true)
        .build();
    hb.pack_end(&menu);
    let refresh = gtk::Button::builder().icon_name("view-refresh-symbolic").tooltip_text("Refresh (F5)").build();
    hb.pack_end(&refresh);
    let upd = updates::View::new();
    hb.pack_end(&upd.update_all);
    let select =
        gtk::ToggleButton::builder().label("Select").tooltip_text("Select several apps to uninstall together").build();
    hb.pack_end(&select);
    let select_all =
        gtk::Button::builder().label("Select all").visible(false).tooltip_text("Tick every app shown (Ctrl+A)").build();
    hb.pack_end(&select_all);
    let sort_dd = gtk::DropDown::from_strings(&["Name", "Size (largest first)"]);
    sort_dd.set_tooltip_text(Some("Sort by"));
    hb.pack_end(&sort_dd);
    tv.add_top_bar(&hb);
    let unsupported = adw::Banner::new(
        "Depot can't read this system's own packages yet, so apps that came with it may show up as manual installs.",
    );
    tv.add_top_bar(&unsupported);

    // the selection bar, slid in from the bottom in selection mode
    let bar = gtk::ActionBar::new();
    let sel_label = widgets::label("", &["heading"]);
    bar.pack_start(&sel_label);
    let sel_remove = gtk::Button::with_label("Uninstall");
    sel_remove.add_css_class("destructive-action");
    sel_remove.add_css_class("pill");
    bar.pack_end(&sel_remove);
    tv.add_bottom_bar(&bar);
    tv.set_reveal_bottom_bars(false);

    let selection = select::Selection::new();
    let factory = gtk::SignalListItemFactory::new();
    {
        let selection = selection.clone();
        factory.connect_setup(move |_, li| {
            let card = widgets::card();
            selection.setup(&card);
            li.downcast_ref::<gtk::ListItem>().expect("ListItem").set_child(Some(&card));
        });
    }
    {
        let selection = selection.clone();
        factory.connect_bind(move |_, li| {
            let li = li.downcast_ref::<gtk::ListItem>().expect("ListItem");
            if let (Some(obj), Some(card)) = (li.item(), li.child().and_downcast::<gtk::Overlay>()) {
                let it = item(&obj);
                widgets::bind_card(&card, it.p(), &it.others().collect::<Vec<_>>());
                selection.bound(&card, select::key(it.p()));
            }
        });
    }
    let grid = gtk::GridView::builder()
        .model(&gtk::NoSelection::new(Some(sorted.clone())))
        .factory(&factory)
        .single_click_activate(true)
        .min_columns(1)
        .max_columns(8)
        .margin_start(12)
        .margin_end(12)
        .margin_top(12)
        .margin_bottom(12)
        .build();

    let stack =
        gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).transition_duration(200).build();
    stack.add_named(&skeleton(), Some("loading"));
    stack.add_named(
        &gtk::ScrolledWindow::builder().child(&grid).hscrollbar_policy(gtk::PolicyType::Never).build(),
        Some("grid"),
    );
    let empty = adw::StatusPage::builder().icon_name("folder-symbolic").build();
    stack.add_named(&empty, Some("empty"));
    let clear = gtk::Button::builder().label("Clear search").halign(gtk::Align::Center).build();
    clear.add_css_class("pill");
    let no_results = adw::StatusPage::builder().icon_name("system-search-symbolic").child(&clear).build();
    stack.add_named(&no_results, Some("no-results"));
    let retry = gtk::Button::builder().label("Try again").halign(gtk::Align::Center).build();
    retry.add_css_class("pill");
    retry.add_css_class("suggested-action");
    let failed = adw::StatusPage::builder()
        .icon_name("dialog-error-symbolic")
        .title("Couldn't read installed software")
        .child(&retry)
        .build();
    stack.add_named(&failed, Some("failed"));
    let view =
        gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).transition_duration(200).build();
    view.add_named(&stack, Some("browse"));
    view.add_named(&upd.root, Some("updates"));
    tv.set_content(Some(&view));
    split.set_content(Some(&tv));

    // narrow windows: the sidebar slides over the grid instead of squeezing it
    let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        860.0,
        adw::LengthUnit::Sp,
    ));
    narrow.add_setter(&split, "collapsed", Some(&true.to_value()));
    narrow.add_setter(&toggle, "active", Some(&false.to_value()));
    // phones: the sort menu doesn't fit next to the search box
    let phone = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        500.0,
        adw::LengthUnit::Sp,
    ));
    phone.add_setter(&split, "collapsed", Some(&true.to_value()));
    phone.add_setter(&toggle, "active", Some(&false.to_value()));
    phone.add_setter(&sort_dd, "visible", Some(&false.to_value()));
    win.add_breakpoint(narrow);
    win.add_breakpoint(phone);

    let main_page = adw::NavigationPage::new(&split, "Depot");
    nav.add(&main_page);

    let st = Rc::new(State {
        win: win.clone(),
        toasts,
        nav,
        main_page,
        store,
        filtered: filtered.clone(),
        stack,
        empty,
        no_results,
        failed,
        search: search.clone(),
        loading: Cell::new(true),
        scanning: Cell::new(false),
        rescan: Cell::new(false),
        rows,
        show_advanced: Cell::new(false),
        cat,
        packages: RefCell::new(Rc::new(Vec::new())),
        updates: upd,
        unsupported,
        view,
        browse_tools: vec![search.clone().upcast(), sort_dd.clone().upcast(), select.clone().upcast()],
        content: tv,
        selection,
        select: select.clone(),
        select_all: select_all.clone(),
        sel_label,
        sel_remove: sel_remove.clone(),
    });
    select::connect(&st);
    about::install(&st);

    // ---- signals
    {
        let st = st.clone();
        filtered.connect_items_changed(move |_, _, _, _| update_stack(&st));
    }
    {
        let filter = filter.clone();
        let st = st.clone();
        search.connect_search_changed(move |e| {
            *query.borrow_mut() = e.text().trim().to_lowercase();
            filter.changed(gtk::FilterChange::Different);
            update_stack(&st);
        });
    }
    {
        let search = search.clone();
        clear.connect_clicked(move |_| search.set_text(""));
    }
    {
        let st = st.clone();
        retry.connect_clicked(move |_| reload(&st));
    }
    {
        let st = st.clone();
        catlist.connect_row_activated(move |_, row| {
            if st.rows.iter().any(|r| &r.row == row && r.nav == Nav::Advanced) {
                st.show_advanced.set(!st.show_advanced.get());
                update_rows(&st);
            }
        });
    }
    {
        let st = st.clone();
        let split = split.clone();
        catlist.connect_row_selected(move |_, row| {
            let Some(row) = row else { return };
            let Some(nav) = st.rows.iter().find(|r| &r.row == row).map(|r| r.nav) else { return };
            let on_updates = nav == Nav::Updates;
            if on_updates {
                select::set_mode(&st, false);
            }
            st.view.set_visible_child_name(if on_updates { "updates" } else { "browse" });
            for w in &st.browse_tools {
                w.set_visible(!on_updates);
            }
            st.updates.sync_ticks(on_updates);
            match nav {
                Nav::Cat(i) => {
                    st.cat.set(i);
                    filter.changed(gtk::FilterChange::Different);
                    update_stack(&st);
                }
                Nav::Updates if st.updates.is_stale() => updates::check(&st, true),
                _ => {}
            }
            if split.is_collapsed() {
                split.set_show_sidebar(false);
            }
        });
    }
    catlist.select_row(Some(&st.rows[0].row));
    sort_dd.connect_selected_notify(move |dd| sorted.set_sorter(Some(&sorter(dd.selected()))));
    {
        let st = st.clone();
        refresh.connect_clicked(move |_| refresh_view(&st));
    }
    {
        let st = st.clone();
        st.clone().updates.update_all.connect_clicked(move |_| {
            let picked = st.updates.selected();
            updates::run(&st, picked);
        });
    }
    {
        let st = st.clone();
        select.connect_toggled(move |b| select::set_mode(&st, b.is_active()));
    }
    {
        let st = st.clone();
        select_all.connect_clicked(move |_| select::select_all(&st));
    }
    {
        let st = st.clone();
        sel_remove.connect_clicked(move |_| select::remove_picked(&st));
    }
    {
        let st = st.clone();
        grid.connect_activate(move |g, pos| {
            if let Some(obj) = g.model().and_then(|m| m.item(pos)) {
                let it = item(&obj);
                let (p, others) = (it.p().clone(), it.others().cloned().collect());
                drop(it);
                if st.selection.on.get() {
                    select::toggle(&st, &p);
                } else {
                    detail::show(&st, p, others);
                }
            }
        });
    }
    shortcuts(&st, &win);

    update_rows(&st);
    updates::tick_clock(&st);
    win.present();
    reload(&st);
}

/// Refresh the page that's showing: re-check updates, or re-read what's installed.
fn refresh_view(st: &Rc<State>) {
    if st.on_updates() {
        updates::check(st, true);
    } else {
        reload(st);
    }
}

fn shortcuts(st: &Rc<State>, win: &adw::ApplicationWindow) {
    let ctrl = gtk::ShortcutController::new();
    ctrl.set_scope(gtk::ShortcutScope::Global);
    let add = |trigger: &str, f: Box<dyn Fn() -> bool>| {
        ctrl.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string(trigger),
            Some(gtk::CallbackAction::new(
                move |_, _| {
                    if f() { glib::Propagation::Stop } else { glib::Propagation::Proceed }
                },
            )),
        ));
    };
    {
        let st = st.clone();
        add(
            "<Control>f",
            Box::new(move || {
                st.search.grab_focus();
                true
            }),
        );
    }
    for key in ["F5", "<Control>r"] {
        let st = st.clone();
        add(
            key,
            Box::new(move || {
                refresh_view(&st);
                true
            }),
        );
    }
    {
        let st = st.clone();
        add(
            "<Control>a",
            Box::new(move || {
                // in the search box, Ctrl+A selects its text as usual
                if !st.selection.on.get() || st.search.has_focus() {
                    return false;
                }
                select::select_all(&st);
                true
            }),
        );
    }
    {
        let st = st.clone();
        add(
            "Escape",
            Box::new(move || {
                let on_main = st.nav.visible_page().is_some_and(|p| p == st.main_page);
                if !on_main {
                    st.nav.pop();
                    true
                } else if st.selection.on.get() {
                    select::set_mode(&st, false);
                    true
                } else {
                    false
                }
            }),
        );
    }
    win.add_controller(ctrl);
}

fn sorter(mode: u32) -> gtk::CustomSorter {
    if mode == 1 {
        gtk::CustomSorter::new(|a, b| item(b).p().size.cmp(&item(a).p().size).into())
    } else {
        gtk::CustomSorter::new(|a, b| item(a).sort.cmp(&item(b).sort).into())
    }
}

/// Grid, or the right empty page: nothing in this category, or nothing matching the search.
fn update_stack(st: &State) {
    if st.loading.get() || st.stack.visible_child_name().as_deref() == Some("failed") && st.store.n_items() == 0 {
        return;
    }
    if st.filtered.n_items() > 0 {
        st.stack.set_visible_child_name("grid");
        return;
    }
    let q = st.search.text();
    let q = q.trim();
    if q.is_empty() {
        let c = &CATS[st.cat.get()];
        let title =
            if st.cat.get() == 0 { "No apps found".to_string() } else { format!("No {} here", c.name.to_lowercase()) };
        st.empty.set_title(&title);
        st.empty.set_description(Some(c.empty));
        st.empty.set_icon_name(Some(c.icon));
        st.stack.set_visible_child_name("empty");
    } else {
        st.no_results.set_title(&format!("No results for “{q}”"));
        st.no_results.set_description(Some(&format!(
            "Nothing in {} matches. Try a shorter word, or look in All apps.",
            CATS[st.cat.get()].name
        )));
        st.stack.set_visible_child_name("no-results");
    }
}

/// Show or hide sidebar rows: advanced ones follow the toggle, empty ones hide (except "All apps").
/// Rows slide in and out rather than jumping.
fn update_rows(st: &State) {
    let adv = st.show_advanced.get();
    for r in &st.rows {
        let show = match r.nav {
            Nav::Updates => true,
            Nav::Advanced => {
                if let Some(c) = &r.chevron {
                    if adv {
                        c.add_css_class("open");
                    } else {
                        c.remove_css_class("open");
                    }
                }
                true
            }
            Nav::Cat(i) => {
                let empty = !st.loading.get() && r.count.text() == "0";
                let selected = i == st.cat.get();
                selected || ((!CATS[i].advanced || adv) && (i == 0 || !empty))
            }
        };
        if show {
            r.row.set_visible(true);
            r.reveal.set_reveal_child(true);
        } else {
            r.reveal.set_reveal_child(false);
        }
    }
}

/// A row whose slide-out finished leaves the list entirely, so it takes no space or focus.
fn hide_when_collapsed(r: &SideRow) {
    let row = r.row.clone();
    r.reveal.connect_child_revealed_notify(move |rv| {
        if !rv.reveals_child() && !rv.is_child_revealed() {
            row.set_visible(false);
        }
    });
}

pub fn reload(st: &Rc<State>) {
    if st.scanning.replace(true) {
        st.rescan.set(true);
        return;
    }
    st.loading.set(true);
    // a rescan after an uninstall keeps the grid in place instead of flashing placeholders
    if st.store.n_items() == 0 {
        st.stack.set_visible_child_name("loading");
    }
    let st = st.clone();
    glib::spawn_future_local(async move {
        let inv = gio::spawn_blocking(|| depot_core::scan(&Env::system())).await;
        st.scanning.set(false);
        if st.rescan.take() {
            // something changed while this scan ran (e.g. an uninstall finished); read it again
            reload(&st);
            return;
        }
        match inv {
            Ok(inv) => loaded(&st, inv),
            Err(_) => {
                st.loading.set(false);
                st.failed.set_description(Some(
                    "Something went wrong while Depot was looking at your computer. If it keeps happening, \
                     please report it: the guide's \"Reporting a problem\" section says how.",
                ));
                st.stack.set_visible_child_name("failed");
            }
        }
    });
}

/// Show the number of pending updates next to "Updates", with a little bounce when it changes.
fn update_counts(st: &State) {
    let n = st.updates.count();
    for r in &st.rows {
        if r.nav == Nav::Updates {
            let text = if n == 0 { String::new() } else { n.to_string() };
            if r.count.text() != text {
                r.count.set_text(&text);
                bump(&r.count);
            }
        }
    }
    st.updates.sync_ticks(st.on_updates());
}

/// Replay the "count changed" animation on a label.
fn bump(l: &gtk::Label) {
    l.remove_css_class("bump");
    l.add_css_class("bump");
    let l = l.downgrade();
    glib::timeout_add_local_once(std::time::Duration::from_millis(450), move || {
        if let Some(l) = l.upgrade() {
            l.remove_css_class("bump");
        }
    });
}

fn loaded(st: &Rc<State>, inv: Inventory) {
    let mut counts = vec![0usize; CATS.len()];
    st.unsupported.set_revealed(!inv.system_packages);
    let all = Rc::new(inv.packages);
    *st.packages.borrow_mut() = all.clone();
    // an app installed several ways gets one card, under its best copy
    let mut also: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut hidden: HashSet<usize> = HashSet::new();
    for g in inv.duplicates {
        hidden.extend(&g[1..]);
        also.insert(g[0], g[1..].to_vec());
    }
    let items: Vec<glib::BoxedAnyObject> = (0..all.len())
        .filter(|i| !hidden.contains(i))
        .map(|i| {
            let it = Item {
                all: all.clone(),
                i,
                also: also.remove(&i).unwrap_or_default(),
                search: String::new(),
                sort: String::new(),
            };
            for (n, c) in CATS.iter().enumerate() {
                if it.matches(c.pred) {
                    counts[n] += 1;
                }
            }
            let search =
                std::iter::once(it.p()).chain(it.others()).map(Package::search_text).collect::<Vec<_>>().join(" ");
            let sort = it.p().title.to_lowercase();
            glib::BoxedAnyObject::new(Item { search, sort, ..it })
        })
        .collect();
    st.store.splice(0, st.store.n_items(), &items);
    for r in &st.rows {
        if let Nav::Cat(i) = r.nav {
            r.count.set_text(&counts[i].to_string());
        }
    }
    st.loading.set(false);
    update_rows(st);
    update_stack(st);
    select::prune(st);
    for w in inv.warnings {
        st.toast(&w);
    }
    // keep the Updates count current. Asking the stores is slow and uses the network, so only
    // when asked for or when the last answer is old; otherwise just drop what's no longer installed.
    match st.updates.take_queued() {
        Some(refresh) => updates::check(st, refresh),
        None if st.updates.is_stale() => updates::check(st, false),
        None => updates::prune(st),
    }
}
