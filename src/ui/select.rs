//! Selection mode: tick several apps and uninstall them together.
//!
//! "Select" in the header bar (or Ctrl+click, or a long press on a card) turns it on. Cards then
//! show a tick box and clicking one ticks it instead of opening it; the bar at the bottom says
//! how many are ticked and uninstalls them in one go. Esc or "Cancel" turns it off.

use super::{State, item, remove, widgets};
use adw::prelude::*;
use depot_core::Package;
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// Identifies one copy of an app across rescans.
pub fn key(p: &Package) -> String {
    format!("{}/{}/{}", p.kind.css(), p.id, p.system)
}

type Pick = Rc<dyn Fn(String)>;

pub struct Selection {
    pub on: Cell<bool>,
    keys: RefCell<HashSet<String>>,
    /// Every card the grid made, with the key of the app it shows now.
    cards: RefCell<HashMap<gtk::Overlay, String>>,
    /// Called with a card's key when Ctrl+click or a long press asks to tick it.
    pick: RefCell<Option<Pick>>,
}

impl Selection {
    pub fn new() -> Rc<Selection> {
        Rc::new(Selection {
            on: Cell::new(false),
            keys: RefCell::new(HashSet::new()),
            cards: RefCell::new(HashMap::new()),
            pick: RefCell::new(None),
        })
    }

    /// A card was just made: Ctrl+click or a long press on it starts a selection with it.
    pub fn setup(self: &Rc<Self>, card: &gtk::Overlay) {
        let click = gtk::GestureClick::builder().button(1).propagation_phase(gtk::PropagationPhase::Capture).build();
        {
            let (sel, card) = (Rc::downgrade(self), card.downgrade());
            click.connect_pressed(move |g, _, _, _| {
                let ctrl = g.current_event_state().contains(gtk::gdk::ModifierType::CONTROL_MASK);
                match (ctrl, sel.upgrade(), card.upgrade()) {
                    (true, Some(sel), Some(card)) => {
                        g.set_state(gtk::EventSequenceState::Claimed);
                        sel.request(&card);
                    }
                    // a plain click belongs to the grid, which opens or ticks the card
                    _ => {
                        g.set_state(gtk::EventSequenceState::Denied);
                    }
                }
            });
        }
        card.add_controller(click);
        // touchscreens only: with a mouse it would hold back the grid's own clicks
        let hold = gtk::GestureLongPress::builder().touch_only(true).build();
        {
            let (sel, card) = (Rc::downgrade(self), card.downgrade());
            hold.connect_pressed(move |g, _, _| {
                if let (Some(sel), Some(card)) = (sel.upgrade(), card.upgrade())
                    && !sel.on.get()
                {
                    g.set_state(gtk::EventSequenceState::Claimed);
                    sel.request(&card);
                }
            });
        }
        card.add_controller(hold);
    }

    fn request(&self, card: &gtk::Overlay) {
        let k = self.cards.borrow().get(card).cloned();
        let f = self.pick.borrow().clone();
        if let (Some(k), Some(f)) = (k, f) {
            f(k);
        }
    }

    /// A card now shows the app with key `k`.
    pub fn bound(&self, card: &gtk::Overlay, k: String) {
        self.show(card, &k);
        self.cards.borrow_mut().insert(card.clone(), k);
    }

    /// The grid threw a card away.
    pub fn forget(&self, card: &gtk::Overlay) {
        self.cards.borrow_mut().remove(card);
    }

    fn show(&self, card: &gtk::Overlay, k: &str) {
        let (bx, tick) = widgets::card_parts(card);
        let picked = self.on.get() && self.keys.borrow().contains(k);
        tick.set_visible(self.on.get());
        tick.set_active(picked);
        if picked {
            bx.add_css_class("selected");
        } else {
            bx.remove_css_class("selected");
        }
    }

    fn sync(&self) {
        for (card, k) in self.cards.borrow().iter() {
            self.show(card, k);
        }
    }

    pub fn count(&self) -> usize {
        self.keys.borrow().len()
    }
}

/// Turn selection mode on or off.
pub fn set_mode(st: &Rc<State>, on: bool) {
    let sel = &st.selection;
    if sel.on.replace(on) == on {
        return;
    }
    if !on {
        sel.keys.borrow_mut().clear();
    }
    st.select.set_active(on);
    st.select.set_label(if on { "Cancel" } else { "Select" });
    st.select.set_tooltip_text(Some(if on {
        "Stop selecting (Esc)"
    } else {
        "Select several apps to uninstall together"
    }));
    st.select_all.set_visible(on);
    st.content.set_reveal_bottom_bars(on);
    sel.sync();
    update_bar(st);
}

/// Tick or untick an app. Apps Depot can't remove say why instead.
pub fn toggle(st: &Rc<State>, p: &Package) {
    if let Some(reason) = &p.blocked {
        st.toast(&format!("{} can't be selected: {}", p.title, reason.trim_end_matches('.')));
        return;
    }
    let k = key(p);
    {
        let mut keys = st.selection.keys.borrow_mut();
        if !keys.remove(&k) {
            keys.insert(k);
        }
    }
    st.selection.sync();
    update_bar(st);
}

/// Tick everything the grid shows now.
pub fn select_all(st: &Rc<State>) {
    let n = st.filtered.n_items();
    let all_ticked = (0..n).filter_map(|i| st.filtered.item(i)).all(|o| {
        let it = item(&o);
        it.p().blocked.is_some() || st.selection.keys.borrow().contains(&key(it.p()))
    });
    {
        let mut keys = st.selection.keys.borrow_mut();
        for o in (0..n).filter_map(|i| st.filtered.item(i)) {
            let it = item(&o);
            if it.p().blocked.is_none() {
                if all_ticked {
                    keys.remove(&key(it.p()));
                } else {
                    keys.insert(key(it.p()));
                }
            }
        }
    }
    st.selection.sync();
    update_bar(st);
}

/// The ticked apps, every copy of each.
fn picked(st: &State) -> Vec<Package> {
    let keys = st.selection.keys.borrow();
    let mut out = Vec::new();
    for o in (0..st.store.n_items()).filter_map(|i| st.store.item(i)) {
        let it = item(&o);
        if keys.contains(&key(it.p())) {
            out.push(it.p().clone());
            out.extend(it.others().filter(|q| q.blocked.is_none()).cloned());
        }
    }
    out
}

fn update_bar(st: &Rc<State>) {
    let pkgs = picked(st);
    let n = st.selection.count();
    let size: u64 = pkgs.iter().map(|p| p.size).sum();
    let mut text = if n == 0 { "Tick the apps you want to remove".to_string() } else { format!("{n} selected") };
    if size > 0 {
        text.push_str(&format!(" · {}", glib::format_size(size)));
    }
    if st.sel_label.text() != text && n > 0 {
        st.win.upcast_ref::<gtk::Widget>().announce(&text, gtk::AccessibleAnnouncementPriority::Medium);
    }
    st.sel_label.set_text(&text);
    st.sel_remove.set_sensitive(n > 0);
    st.sel_remove.set_label(&match n {
        0 | 1 => "Uninstall".to_string(),
        n => format!("Uninstall {n} apps"),
    });
}

/// Uninstall what's ticked.
pub fn remove_picked(st: &Rc<State>) {
    remove::start(st, picked(st), Some(&st.sel_remove));
}

/// After an uninstall: leave selection mode.
pub fn finish(st: &Rc<State>) {
    set_mode(st, false);
}

/// After a rescan: forget ticks of apps that are gone.
pub fn prune(st: &Rc<State>) {
    let present: HashSet<String> =
        (0..st.store.n_items()).filter_map(|i| st.store.item(i)).map(|o| key(item(&o).p())).collect();
    st.selection.keys.borrow_mut().retain(|k| present.contains(k));
    st.selection.sync();
    update_bar(st);
}

/// Let Ctrl+click and long presses on cards reach `st`.
pub fn connect(st: &Rc<State>) {
    let w = Rc::downgrade(st);
    *st.selection.pick.borrow_mut() = Some(Rc::new(move |k: String| {
        let Some(st) = w.upgrade() else { return };
        let found = (0..st.store.n_items())
            .filter_map(|i| st.store.item(i))
            .find(|o| key(item(o).p()) == k)
            .map(|o| item(&o).p().clone());
        if let Some(p) = found {
            set_mode(&st, true);
            toggle(&st, &p);
        }
    }));
}
