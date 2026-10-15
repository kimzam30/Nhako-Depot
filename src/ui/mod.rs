//! Main window: category sidebar, searchable card grid, and navigation to the detail page.

mod detail;
mod remove;
mod widgets;

use adw::prelude::*;
use depot_core::{Env, Inventory, Kind, Package, Role};
use gtk::{gdk, gio, glib};
use std::cell::{Cell, Ref, RefCell};
use std::rc::Rc;

/// A package plus precomputed keys, stored in the list model.
pub struct Item {
    pub p: Package,
    search: String,
    sort: String,
}

pub fn item(o: &glib::Object) -> Ref<'_, Item> {
    o.downcast_ref::<glib::BoxedAnyObject>().expect("list item").borrow::<Item>()
}

struct Cat {
    name: &'static str,
    icon: &'static str,
    advanced: bool,
    pred: fn(&Package) -> bool,
}

const CATS: &[Cat] = &[
    Cat { name: "All apps", icon: "view-grid-symbolic", advanced: false, pred: |p| p.role == Role::App && !p.broken },
    Cat {
        name: "Deb apps",
        icon: "system-software-install-symbolic",
        advanced: false,
        pred: |p| p.role == Role::App && p.kind == Kind::Deb,
    },
    Cat {
        name: "Snap apps",
        icon: "package-x-generic-symbolic",
        advanced: false,
        pred: |p| p.role == Role::App && p.kind == Kind::Snap,
    },
    Cat {
        name: "Flatpak apps",
        icon: "view-app-grid-symbolic",
        advanced: false,
        pred: |p| p.role == Role::App && p.kind == Kind::Flatpak,
    },
    Cat {
        name: "Manual installs",
        icon: "folder-download-symbolic",
        advanced: false,
        pred: |p| p.kind == Kind::Manual && !p.broken,
    },
    Cat { name: "Broken launchers", icon: "dialog-warning-symbolic", advanced: false, pred: |p| p.broken },
    Cat {
        name: "Tools you installed",
        icon: "utilities-terminal-symbolic",
        advanced: true,
        pred: |p| p.role == Role::Tool && p.user_installed,
    },
    Cat {
        name: "All system packages",
        icon: "applications-engineering-symbolic",
        advanced: true,
        pred: |p| p.role == Role::Tool,
    },
    Cat {
        name: "Runtimes & bases",
        icon: "applications-system-symbolic",
        advanced: true,
        pred: |p| p.role == Role::Runtime,
    },
];

pub struct State {
    pub win: adw::ApplicationWindow,
    pub toasts: adw::ToastOverlay,
    pub nav: adw::NavigationView,
    pub main_page: adw::NavigationPage,
    store: gio::ListStore,
    filtered: gtk::FilterListModel,
    stack: gtk::Stack,
    loading: Cell<bool>,
    /// Sidebar rows; `None` marks the "Advanced" toggle row.
    rows: Vec<(gtk::ListBoxRow, Option<usize>, gtk::Label)>,
    show_advanced: Cell<bool>,
    cat: Rc<Cell<usize>>,
}

impl State {
    pub fn toast(&self, text: &str) {
        self.toasts.add_toast(adw::Toast::new(text));
    }
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
            (CATS[cat.get()].pred)(&it.p) && query.borrow().split_whitespace().all(|w| it.search.contains(w))
        })
    };
    let filtered = gtk::FilterListModel::new(Some(store.clone()), Some(filter.clone()));
    let sorted = gtk::SortListModel::new(Some(filtered.clone()), Some(sorter(0)));

    let win = adw::ApplicationWindow::builder()
        .application(app)
        .title("Depot")
        .default_width(1100)
        .default_height(720)
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
    let mut add_row = |name: &str, icon: &str, idx: Option<usize>| {
        let row = gtk::ListBoxRow::new();
        let bx = gtk::Box::builder().spacing(10).margin_top(8).margin_bottom(8).margin_start(6).margin_end(6).build();
        bx.append(&gtk::Image::from_icon_name(icon));
        bx.append(
            &gtk::Label::builder()
                .label(name)
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build(),
        );
        let cnt = widgets::label("", &["dim-label", "caption"]);
        bx.append(&cnt);
        row.set_child(Some(&bx));
        if idx.is_none() {
            row.set_selectable(false);
            row.set_tooltip_text(Some("Libraries, command-line tools and runtimes. Most people never need these."));
        }
        catlist.append(&row);
        rows.push((row, idx, cnt));
    };
    for (i, c) in CATS.iter().enumerate().filter(|(_, c)| !c.advanced) {
        add_row(c.name, c.icon, Some(i));
    }
    add_row("Advanced", "pan-end-symbolic", None);
    for (i, c) in CATS.iter().enumerate().filter(|(_, c)| c.advanced) {
        add_row(c.name, c.icon, Some(i));
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
    let refresh = gtk::Button::builder().icon_name("view-refresh-symbolic").tooltip_text("Refresh").build();
    hb.pack_end(&refresh);
    let sort_dd = gtk::DropDown::from_strings(&["Name", "Size (largest first)"]);
    sort_dd.set_tooltip_text(Some("Sort by"));
    hb.pack_end(&sort_dd);
    tv.add_top_bar(&hb);

    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, li| {
        li.downcast_ref::<gtk::ListItem>().expect("ListItem").set_child(Some(&widgets::card()));
    });
    factory.connect_bind(|_, li| {
        let li = li.downcast_ref::<gtk::ListItem>().expect("ListItem");
        if let (Some(obj), Some(card)) = (li.item(), li.child()) {
            widgets::bind_card(&card, &item(&obj).p);
        }
    });
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

    let stack = gtk::Stack::new();
    let spinner = gtk::Spinner::builder().spinning(true).width_request(32).height_request(32).build();
    stack.add_named(
        &adw::StatusPage::builder().title("Reading installed software…").child(&spinner).build(),
        Some("loading"),
    );
    stack.add_named(
        &gtk::ScrolledWindow::builder().child(&grid).hscrollbar_policy(gtk::PolicyType::Never).build(),
        Some("grid"),
    );
    stack.add_named(
        &adw::StatusPage::builder()
            .icon_name("system-search-symbolic")
            .title("Nothing found")
            .description("No installed software matches here.")
            .build(),
        Some("empty"),
    );
    tv.set_content(Some(&stack));
    split.set_content(Some(&tv));

    // narrow windows: the sidebar slides over the grid instead of squeezing it
    let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        860.0,
        adw::LengthUnit::Sp,
    ));
    narrow.add_setter(&split, "collapsed", Some(&true.to_value()));
    narrow.add_setter(&toggle, "active", Some(&false.to_value()));
    win.add_breakpoint(narrow);

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
        loading: Cell::new(true),
        rows,
        show_advanced: Cell::new(false),
        cat,
    });

    // ---- signals
    {
        let st = st.clone();
        filtered.connect_items_changed(move |_, _, _, _| update_stack(&st));
    }
    {
        let filter = filter.clone();
        search.connect_search_changed(move |e| {
            *query.borrow_mut() = e.text().trim().to_lowercase();
            filter.changed(gtk::FilterChange::Different);
        });
    }
    {
        let st = st.clone();
        catlist.connect_row_activated(move |_, row| {
            if st.rows.iter().any(|(r, idx, _)| r == row && idx.is_none()) {
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
            if let Some((_, Some(i), _)) = st.rows.iter().find(|(r, _, _)| r == row) {
                st.cat.set(*i);
                filter.changed(gtk::FilterChange::Different);
                if split.is_collapsed() {
                    split.set_show_sidebar(false);
                }
            }
        });
    }
    catlist.select_row(Some(&st.rows[0].0));
    sort_dd.connect_selected_notify(move |dd| sorted.set_sorter(Some(&sorter(dd.selected()))));
    {
        let st = st.clone();
        refresh.connect_clicked(move |_| reload(&st));
    }
    {
        let st = st.clone();
        grid.connect_activate(move |g, pos| {
            if let Some(obj) = g.model().and_then(|m| m.item(pos)) {
                let p = item(&obj).p.clone();
                detail::show(&st, p);
            }
        });
    }
    let ctrl_f = gtk::ShortcutController::new();
    {
        let search = search.clone();
        ctrl_f.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control>f"),
            Some(gtk::CallbackAction::new(move |_, _| {
                search.grab_focus();
                glib::Propagation::Stop
            })),
        ));
    }
    win.add_controller(ctrl_f);

    update_rows(&st);
    win.present();
    reload(&st);
}

fn sorter(mode: u32) -> gtk::CustomSorter {
    if mode == 1 {
        gtk::CustomSorter::new(|a, b| item(b).p.size.cmp(&item(a).p.size).into())
    } else {
        gtk::CustomSorter::new(|a, b| item(a).sort.cmp(&item(b).sort).into())
    }
}

fn update_stack(st: &State) {
    if st.loading.get() {
        return;
    }
    st.stack.set_visible_child_name(if st.filtered.n_items() > 0 { "grid" } else { "empty" });
}

/// Show or hide sidebar rows: advanced ones follow the toggle, empty ones hide (except "All apps").
fn update_rows(st: &State) {
    let adv = st.show_advanced.get();
    for (row, idx, cnt) in &st.rows {
        match idx {
            None => {
                if let Some(img) = row.child().and_then(|b| b.first_child()).and_downcast::<gtk::Image>() {
                    img.set_icon_name(Some(if adv { "pan-down-symbolic" } else { "pan-end-symbolic" }));
                }
            }
            Some(i) => {
                let empty = !st.loading.get() && cnt.text() == "0";
                let selected = *i == st.cat.get();
                row.set_visible(selected || ((!CATS[*i].advanced || adv) && (*i == 0 || !empty)));
            }
        }
    }
}

pub fn reload(st: &Rc<State>) {
    st.loading.set(true);
    st.stack.set_visible_child_name("loading");
    let st = st.clone();
    glib::spawn_future_local(async move {
        let inv = gio::spawn_blocking(|| depot_core::scan(&Env::system())).await.unwrap_or_default();
        loaded(&st, inv);
    });
}

fn loaded(st: &State, inv: Inventory) {
    let mut counts = vec![0usize; CATS.len()];
    let items: Vec<glib::BoxedAnyObject> = inv
        .packages
        .into_iter()
        .map(|p| {
            for (i, c) in CATS.iter().enumerate() {
                if (c.pred)(&p) {
                    counts[i] += 1;
                }
            }
            let search = p.search_text();
            let sort = p.title.to_lowercase();
            glib::BoxedAnyObject::new(Item { p, search, sort })
        })
        .collect();
    st.store.splice(0, st.store.n_items(), &items);
    for (_, idx, cnt) in &st.rows {
        if let Some(i) = idx {
            cnt.set_text(&counts[*i].to_string());
        }
    }
    st.loading.set(false);
    update_rows(st);
    update_stack(st);
    for w in inv.warnings {
        st.toast(&w);
    }
}
