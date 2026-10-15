#!/usr/bin/env python3
"""Depot: one place to browse and uninstall everything installed: deb, snap, flatpak and manual installs."""
import glob
import http.client
import json
import os
import re
import shlex
import shutil
import socket
import subprocess
import sys
import threading

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
from gi.repository import Adw, Gio, GLib, GObject, Gtk, Pango  # noqa: E402

APP_ID = "dev.kimzam.Depot"
C_ENV = {**os.environ, "LC_ALL": "C.UTF-8", "LANGUAGE": "C"}
OWNED_DESKTOPS = set()
HOME = os.path.expanduser("~")
OPT = HOME + "/.local/opt"
PROTECTED_SNAP_TYPES = {"base", "os", "snapd", "core", "kernel", "gadget"}

CSS = b"""
.pkg-card { padding: 16px 14px 12px 14px; }
.pkg-card:hover { background: alpha(currentColor, .06); }
.badge { border-radius: 99px; padding: 0 8px; font-size: 0.75em; font-weight: 700; }
.badge-snap { background: alpha(#e95420, .18); color: #e95420; }
.badge-deb { background: alpha(#d70a53, .16); color: #d70a53; }
.badge-flatpak { background: alpha(#4a90d9, .18); color: #4a90d9; }
.badge-manual { background: alpha(#2ec27e, .18); color: #2ec27e; }
.badge-sys { background: alpha(currentColor, .10); }
"""


class Pkg(GObject.Object):
    def __init__(self, **kw):
        super().__init__()
        self.kind = "deb"
        self.pkgid = self.name = self.title = ""
        self.version = self.summary = self.description = ""
        self.size = 0
        self.icon = None
        self.desktop = None
        self.is_app = False
        self.removable = True
        self.reason = ""
        self.publisher = self.section = self.homepage = self.channel = ""
        self.revision = self.location = ""
        self.plan, self.data, self.appdir = [], [], None
        self.__dict__.update(kw)
        self.search = f"{self.title} {self.name} {self.summary}".lower()


# ---------------------------------------------------------------- data layer
def desktop_info(path):
    try:
        info = Gio.DesktopAppInfo.new_from_filename(path)
    except Exception:
        return None
    return info if info and info.should_show() else None


def load_debs():
    fmt = "${binary:Package}\t${Package}\t${Version}\t${Installed-Size}\t${db:Status-Abbrev}\t" \
          "${Essential}\t${Section}\t${Homepage}\t${binary:Summary}\n"
    out = subprocess.run(["dpkg-query", "-W", "-f", fmt], capture_output=True, text=True, env=C_ENV).stdout

    # package -> first visible .desktop file shipped by it
    global OWNED_DESKTOPS
    desk = {}
    OWNED_DESKTOPS = set()
    for lst in glob.glob("/var/lib/dpkg/info/*.list"):
        try:
            with open(lst, errors="ignore") as fh:
                files = [l.strip() for l in fh if l.startswith("/usr/share/applications/") and l.rstrip().endswith(".desktop")]
        except OSError:
            continue
        OWNED_DESKTOPS.update(files)
        for f in files:
            info = desktop_info(f)
            if info:
                desk[os.path.basename(lst)[:-5]] = (f, info)
                break

    pkgs, seen = [], set()
    for line in out.splitlines():
        p = line.split("\t")
        if len(p) < 9 or not p[4].startswith("ii") or p[0] in seen:
            continue
        seen.add(p[0])
        pkgid, name, ver, kb, _, ess, sect, home, summ = p[:9]
        hit = desk.get(pkgid) or desk.get(name)
        kw = dict(kind="deb", pkgid=pkgid, name=name, title=name, version=ver,
                  size=int(kb or 0) * 1024, summary=summ, section=sect, homepage=home)
        if ess == "yes":
            kw.update(removable=False, reason="This package is essential for the system.")
        if hit:
            f, info = hit
            kw.update(is_app=True, title=info.get_name() or name, icon=info.get_icon(), desktop=f)
        pkgs.append(Pkg(**kw))
    return pkgs


class _Unix(http.client.HTTPConnection):
    def connect(self):
        self.sock = socket.socket(socket.AF_UNIX)
        self.sock.connect("/run/snapd.socket")


def load_snaps():
    try:
        c = _Unix("localhost")
        c.request("GET", "/v2/snaps")
        data = json.load(c.getresponse())["result"]
    except Exception as e:  # snapd missing / not running
        print("snapd unavailable:", e, file=sys.stderr)
        return []
    pkgs = []
    for s in data:
        name, typ = s["name"], s.get("type", "app")
        icon, desktop = None, None
        for f in sorted(glob.glob(f"/var/lib/snapd/desktop/applications/{name}_*.desktop")):
            info = desktop_info(f)
            if info:
                desktop, icon = f, info.get_icon()
                break
        if icon is None:
            for ext in ("png", "svg"):
                path = f"/snap/{name}/current/meta/gui/icon.{ext}"
                if os.path.exists(path):
                    icon = Gio.FileIcon.new(Gio.File.new_for_path(path))
                    break
        pub = s.get("publisher") or {}
        kw = dict(kind="snap", pkgid=name, name=name, title=s.get("title") or name,
                  version=s.get("version", ""), size=s.get("installed-size", 0),
                  summary=s.get("summary", ""), description=s.get("description", ""),
                  icon=icon, desktop=desktop, is_app=bool(desktop and typ == "app"),
                  publisher=pub.get("display-name", "") + (" ✔" if pub.get("validation") == "verified" else ""),
                  homepage=s.get("website") or "", channel=s.get("tracking-channel", ""),
                  revision=s.get("revision", ""), snap_type=typ)
        if typ in PROTECTED_SNAP_TYPES:
            kw.update(removable=False, reason="This is a core snap that other snaps and snapd depend on.")
        pkgs.append(Pkg(**kw))
    return pkgs


FP_SHARE = ["/var/lib/flatpak/exports/share", HOME + "/.local/share/flatpak/exports/share"]


def fp_size(txt):
    m = re.match(r"([\d.,]+)\s*(bytes|kB|MB|GB|TB)", txt.replace("\u00a0", " ").strip())
    if not m:
        return 0
    return int(float(m.group(1).replace(",", "")) * {"bytes": 1, "kB": 1e3, "MB": 1e6, "GB": 1e9, "TB": 1e12}[m.group(2)])


def fp_icon(appid):
    for base in FP_SHARE:
        for sub in ("scalable", "512x512", "256x256", "128x128", "64x64", "48x48"):
            for ext in ("svg", "png"):
                f = f"{base}/icons/hicolor/{sub}/apps/{appid}.{ext}"
                if os.path.exists(f):
                    return Gio.FileIcon.new(Gio.File.new_for_path(f))
    return None


def load_flatpaks():
    if not shutil.which("flatpak"):
        return []
    pkgs = []
    for runtime in (False, True):
        out = subprocess.run(["flatpak", "list", "--runtime" if runtime else "--app",
                              "--columns=application,name,version,branch,description,origin,installation,size"],
                             capture_output=True, text=True, env=C_ENV).stdout
        for line in out.splitlines():
            p = line.split("\t")
            if len(p) < 8:
                continue
            appid, name, ver, branch, desc, origin, inst, size = [x.strip() for x in p[:8]]
            desktop = next((f for f in (f"{b}/applications/{appid}.desktop" for b in FP_SHARE)
                            if os.path.exists(f) and desktop_info(f)), None)
            kw = dict(kind="flatpak", pkgid=f"{appid}//{branch}", name=appid, title=name or appid, version=ver,
                      summary=desc, size=fp_size(size), icon=fp_icon(appid), desktop=desktop,
                      is_app=not runtime, channel=origin, section=inst, revision=branch)
            if runtime:
                kw.update(removable=False, reason="Runtimes are shared by Flatpak apps. Flatpak removes unused ones with: flatpak uninstall --unused")
            pkgs.append(Pkg(**kw))
    return pkgs


def tree_size(path):
    total = 0
    for root, _, files in os.walk(path):
        for f in files:
            try:
                total += os.lstat(os.path.join(root, f)).st_size
            except OSError:
                pass
    return total


def manual_plan(f, info):
    """Work out what belongs to a manually installed app. Returns (appdir, paths, data_paths)."""
    appdir = None
    for tok in shlex.split(info.get_commandline() or ""):
        if tok.startswith(OPT + "/"):
            appdir = OPT + "/" + tok[len(OPT) + 1:].split("/")[0]
            break
    base = os.path.basename(f)[:-8]
    icon = info.get_string("Icon") or ""
    paths = [f]
    if appdir:
        paths.append(appdir)
        for extra in (f"{HOME}/.config/autostart/{base}.desktop", f"{HOME}/.local/share/metainfo/{base}.metainfo.xml"):
            if os.path.exists(extra):
                paths.append(extra)
        for b in glob.glob(HOME + "/.local/bin/*"):
            try:
                hit = os.path.realpath(b).startswith(appdir + "/")
                if not hit and os.path.isfile(b) and os.path.getsize(b) < 8192:
                    hit = appdir in open(b, errors="ignore").read()
            except OSError:
                continue
            if hit:
                paths.append(b)
        if icon and "/" not in icon:
            paths += glob.glob(f"{HOME}/.local/share/icons/**/{icon}.*", recursive=True)
    data = [p for p in (f"{HOME}/.local/share/{base}", f"{HOME}/.config/{base}", f"{HOME}/.cache/{base}") if os.path.exists(p)]
    return appdir, paths, data


def load_manual():
    """Apps with a launcher that no deb, snap or flatpak owns (tarballs, AppImages, /opt installs, ...)."""
    pkgs = []
    # a user-level launcher with the same file name as a deb's launcher is just an override of that deb
    seen = {os.path.basename(f) for f in OWNED_DESKTOPS}
    for d in (HOME + "/.local/share/applications", "/usr/local/share/applications", "/usr/share/applications"):
        for f in sorted(glob.glob(d + "/*.desktop")):
            if os.path.basename(f) in seen:
                continue
            info = desktop_info(f)
            if not info:
                continue
            seen.add(os.path.basename(f))
            base = os.path.basename(f)[:-8]
            appdir, paths, data = manual_plan(f, info)
            cmd = info.get_commandline() or ""
            kw = dict(kind="manual", pkgid=base, name=base, title=info.get_name() or base,
                      summary=info.get_description() or info.get_generic_name() or "",
                      icon=info.get_icon(), desktop=f, is_app=True, location=appdir or cmd,
                      plan=paths, data=data, appdir=appdir)
            if appdir:
                kw["size"] = tree_size(appdir)
                try:
                    kw["version"] = open(appdir + "/.blip-tarball-install").read().strip()
                except OSError:
                    pass
            if "--app-id=" in cmd:
                kw.update(removable=False, reason="This is a web app managed by the browser; remove it from the browser's own app list.")
            elif not f.startswith(HOME):
                kw.update(removable=False, reason="This launcher lives in a system folder and was not installed by apt or snap, so it can't be removed safely from here.")
            pkgs.append(Pkg(**kw))
    return pkgs


def remove_manual(p, with_data):
    """Delete a manual install's files (no root needed). Returns (ok, message)."""
    targets = list(p.plan) + (list(p.data) if with_data else [])
    err = []
    for t in targets:
        rp = os.path.realpath(t) if not os.path.islink(t) else t
        if t == p.appdir and (os.path.dirname(rp) != OPT or rp == OPT):
            err.append(f"refused unsafe path {t}")
            continue
        try:
            if os.path.isdir(t) and not os.path.islink(t):
                shutil.rmtree(t)
            elif os.path.lexists(t):
                os.remove(t)
        except OSError as e:
            err.append(f"{t}: {e}")
    return (not err), "\n".join(err)


def deb_description(pkgid):
    out = subprocess.run(["dpkg-query", "-W", "-f", "${Description}", pkgid],
                         capture_output=True, text=True, env=C_ENV).stdout
    lines = out.split("\n")[1:]
    return "\n".join("" if l.strip() == "." else l.strip() for l in lines).strip()


def simulate_deb_removal(pkgid):
    r = subprocess.run(["apt-get", "-s", "remove", pkgid], capture_output=True, text=True, env=C_ENV)
    base = pkgid.split(":")[0]
    extra = [m for m in re.findall(r"^Remv (\S+)", r.stdout, re.M) if m.split(":")[0] != base]
    blocked = None
    if "essential packages will be removed" in r.stdout:
        blocked = "Removing this would also remove essential system packages."
    elif r.returncode != 0:
        blocked = (r.stderr or r.stdout).strip().splitlines()[-1] if (r.stderr or r.stdout).strip() else "apt refused."
    return extra, blocked


def run_privileged(cmd, root=True):
    pre = ["pkexec", "env", "DEBIAN_FRONTEND=noninteractive", "LC_ALL=C.UTF-8"] if root else []
    return subprocess.run([*pre, *cmd], capture_output=True, text=True)


# ------------------------------------------------------------------ UI parts
def label(text, *classes, wrap=False, **kw):
    l = Gtk.Label(label=text, xalign=0, **kw)
    for c in classes:
        l.add_css_class(c)
    if wrap:
        l.set_wrap(True)
        l.set_wrap_mode(Pango.WrapMode.WORD_CHAR)
    return l


def badge(p):
    l = Gtk.Label(label={"snap": "Snap", "deb": "Deb", "flatpak": "Flatpak", "manual": "Manual"}[p.kind])
    l.add_css_class("badge")
    l.add_css_class("badge-" + p.kind)
    return l


def set_icon(img, p):
    if p.icon is not None:
        img.set_from_gicon(p.icon)
    else:
        img.set_from_icon_name("application-x-executable" if p.is_app else "package-x-generic")


class Card(Gtk.Box):
    def __init__(self):
        super().__init__(orientation=Gtk.Orientation.VERTICAL, spacing=6)
        for c in ("card", "pkg-card"):
            self.add_css_class(c)
        self.set_size_request(210, 190)
        self.img = Gtk.Image(pixel_size=64, halign=Gtk.Align.START)
        self.title = label("", "heading", ellipsize=Pango.EllipsizeMode.END)
        self.summ = label("", "dim-label", "caption", wrap=True, lines=2,
                          ellipsize=Pango.EllipsizeMode.END, valign=Gtk.Align.START, vexpand=True)
        self.badge_box = Gtk.Box(spacing=8)
        self.size = label("", "dim-label", "caption")
        for w in (self.img, self.title, self.summ, self.badge_box):
            self.append(w)


CATS = [
    ("all", "All apps", "view-grid-symbolic", lambda p: p.is_app),
    ("snap", "Snap apps", "package-x-generic-symbolic", lambda p: p.is_app and p.kind == "snap"),
    ("flatpak", "Flatpak apps", "view-app-grid-symbolic", lambda p: p.is_app and p.kind == "flatpak"),
    ("deb", "Deb apps", "system-software-install-symbolic", lambda p: p.is_app and p.kind == "deb"),
    ("manual", "Manual installs", "folder-download-symbolic", lambda p: p.kind == "manual"),
    ("libs", "Deb libraries & tools", "applications-engineering-symbolic", lambda p: p.kind == "deb" and not p.is_app),
    ("rt", "Runtimes & bases", "applications-system-symbolic", lambda p: p.kind in ("snap", "flatpak") and not p.is_app),
]


class Hub(Adw.Application):
    def __init__(self):
        super().__init__(application_id=APP_ID)
        self.cat = CATS[0]
        self.loading = True

    def do_activate(self):
        if getattr(self, "win", None):
            self.win.present()
            return
        Gtk.Window.set_default_icon_name(APP_ID)
        prov = Gtk.CssProvider()
        prov.load_from_data(CSS)
        Gtk.StyleContext.add_provider_for_display(
            __import__("gi.repository.Gdk", fromlist=["Gdk"]).Display.get_default(), prov,
            Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)

        self.store = Gio.ListStore(item_type=Pkg)
        self.filter = Gtk.CustomFilter.new(self.match, None)
        self.filtered = Gtk.FilterListModel(model=self.store, filter=self.filter)
        self.sorted = Gtk.SortListModel(model=self.filtered, sorter=self.make_sorter(0))
        self.filtered.connect("items-changed", lambda *a: self.update_stack())

        self.win = Adw.ApplicationWindow(application=self, title="Depot", default_width=1100, default_height=720)
        self.toasts = Adw.ToastOverlay()
        self.nav = Adw.NavigationView()
        self.toasts.set_child(self.nav)
        self.win.set_content(self.toasts)
        self.main_page = Adw.NavigationPage(title="Depot", child=self.build_main())
        self.nav.add(self.main_page)
        self.win.present()
        self.reload()

    # ---- main view
    def build_main(self):
        self.split = Adw.OverlaySplitView(sidebar_width_fraction=0.22, min_sidebar_width=210)

        # sidebar
        sb = Adw.ToolbarView()
        sb.add_top_bar(Adw.HeaderBar(title_widget=Adw.WindowTitle(title="Depot", subtitle="Everything installed")))
        self.catlist = Gtk.ListBox(selection_mode=Gtk.SelectionMode.SINGLE)
        self.catlist.add_css_class("navigation-sidebar")
        self.count_labels = {}
        for cid, name, icon, _ in CATS:
            row = Gtk.ListBoxRow()
            box = Gtk.Box(spacing=10, margin_top=8, margin_bottom=8, margin_start=6, margin_end=6)
            box.append(Gtk.Image(icon_name=icon))
            box.append(Gtk.Label(label=name, xalign=0, hexpand=True))
            cnt = label("", "dim-label", "caption")
            self.count_labels[cid] = cnt
            box.append(cnt)
            row.set_child(box)
            row.cat = cid
            self.catlist.append(row)
        self.catlist.select_row(self.catlist.get_row_at_index(0))
        self.catlist.connect("row-selected", self.on_cat)
        sb.set_content(Gtk.ScrolledWindow(child=self.catlist, vexpand=True))
        self.split.set_sidebar(sb)

        # content
        tv = Adw.ToolbarView()
        hb = Adw.HeaderBar()
        toggle = Gtk.ToggleButton(icon_name="sidebar-show-symbolic", active=True)
        toggle.bind_property("active", self.split, "show-sidebar", GObject.BindingFlags.BIDIRECTIONAL)
        hb.pack_start(toggle)
        self.search = Gtk.SearchEntry(placeholder_text="Search installed software", hexpand=True)
        self.search.connect("search-changed", lambda *_: self.filter.changed(Gtk.FilterChange.DIFFERENT))
        hb.set_title_widget(Adw.Clamp(maximum_size=420, child=self.search))
        refresh = Gtk.Button(icon_name="view-refresh-symbolic", tooltip_text="Refresh")
        refresh.connect("clicked", lambda *_: self.reload())
        hb.pack_end(refresh)
        self.sort_dd = Gtk.DropDown.new_from_strings(["Name", "Size (largest first)"])
        self.sort_dd.connect("notify::selected", lambda dd, _: self.sorted.set_sorter(self.make_sorter(dd.get_selected())))
        hb.pack_end(self.sort_dd)
        tv.add_top_bar(hb)

        factory = Gtk.SignalListItemFactory()
        factory.connect("setup", lambda f, li: li.set_child(Card()))
        factory.connect("bind", self.bind_card)
        self.grid = Gtk.GridView(model=Gtk.NoSelection(model=self.sorted), factory=factory,
                                 single_click_activate=True, min_columns=1, max_columns=8,
                                 margin_start=12, margin_end=12, margin_top=12, margin_bottom=12)
        self.grid.connect("activate", self.on_activate)

        self.stack = Gtk.Stack()
        spin = Adw.StatusPage(title="Reading installed software…", child=Gtk.Spinner(spinning=True, width_request=32, height_request=32))
        self.stack.add_named(spin, "loading")
        self.stack.add_named(Gtk.ScrolledWindow(child=self.grid, hscrollbar_policy=Gtk.PolicyType.NEVER), "grid")
        self.stack.add_named(Adw.StatusPage(icon_name="system-search-symbolic", title="Nothing found",
                                            description="No installed software matches here."), "empty")
        tv.set_content(self.stack)
        self.split.set_content(tv)
        return self.split

    def make_sorter(self, mode):
        if mode == 1:
            fn = lambda a, b, _: (b.size > a.size) - (b.size < a.size)
        else:
            fn = lambda a, b, _: (a.title.lower() > b.title.lower()) - (a.title.lower() < b.title.lower())
        return Gtk.CustomSorter.new(fn, None)

    def match(self, p, _):
        if not self.cat[3](p):
            return False
        q = self.search.get_text().strip().lower() if hasattr(self, "search") else ""
        return all(w in p.search for w in q.split())

    def on_cat(self, _, row):
        if row:
            self.cat = next(c for c in CATS if c[0] == row.cat)
            self.filter.changed(Gtk.FilterChange.DIFFERENT)

    def update_stack(self):
        if self.loading:
            return
        self.stack.set_visible_child_name("grid" if self.filtered.get_n_items() else "empty")

    def bind_card(self, _, li):
        p, c = li.get_item(), li.get_child()
        set_icon(c.img, p)
        c.title.set_text(p.title)
        c.summ.set_text(p.summary)
        while (ch := c.badge_box.get_first_child()):
            c.badge_box.remove(ch)
        c.badge_box.append(badge(p))
        c.size.set_text(GLib.format_size(p.size) if p.size else "")
        c.badge_box.append(c.size)

    def on_activate(self, grid, pos):
        self.show_detail(grid.get_model().get_item(pos))

    # ---- loading
    def reload(self):
        self.loading = True
        self.stack.set_visible_child_name("loading")

        def work():
            items = load_debs() + load_snaps() + load_flatpaks() + load_manual()
            GLib.idle_add(self.loaded, items)
        threading.Thread(target=work, daemon=True).start()

    def loaded(self, items):
        self.store.splice(0, self.store.get_n_items(), items)
        for cid, _, _, pred in CATS:
            self.count_labels[cid].set_text(str(sum(1 for p in items if pred(p))))
        self.loading = False
        self.update_stack()
        return False

    # ---- detail page
    def show_detail(self, p):
        tv = Adw.ToolbarView()
        tv.add_top_bar(Adw.HeaderBar())
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=20, margin_top=24, margin_bottom=32,
                      margin_start=16, margin_end=16)

        top = Gtk.Box(spacing=20)
        img = Gtk.Image(pixel_size=112, valign=Gtk.Align.START)
        set_icon(img, p)
        top.append(img)
        info = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6, hexpand=True, valign=Gtk.Align.CENTER)
        info.append(label(p.title, "title-1", wrap=True))
        info.append(label(p.summary, "dim-label", wrap=True))
        bb = Gtk.Box(spacing=8, margin_top=2)
        bb.append(badge(p))
        if not p.is_app:
            sys_b = Gtk.Label(label="Library / tool" if p.kind == "deb" else "Runtime")
            sys_b.add_css_class("badge")
            sys_b.add_css_class("badge-sys")
            bb.append(sys_b)
        info.append(bb)
        btns = Gtk.Box(spacing=10, margin_top=10)
        if p.desktop:
            op = Gtk.Button(label="Open")
            op.add_css_class("suggested-action")
            op.add_css_class("pill")
            op.connect("clicked", lambda *_: self.launch(p))
            btns.append(op)
        rm = Gtk.Button(label="Uninstall" if (p.kind != "manual" or p.appdir) else "Remove launcher")
        rm.add_css_class("destructive-action")
        rm.add_css_class("pill")
        if p.removable:
            rm.connect("clicked", lambda b: self.start_uninstall(p, b))
        else:
            rm.set_sensitive(False)
            rm.set_tooltip_text(p.reason)
        btns.append(rm)
        info.append(btns)
        top.append(info)
        box.append(top)
        if not p.removable:
            box.append(label(p.reason, "dim-label", wrap=True))

        desc = label(p.description or "Loading description…", wrap=True, selectable=True)
        if p.kind == "deb" and not p.description:
            def fetch():
                d = deb_description(p.pkgid) or "No description available."
                p.description = d
                GLib.idle_add(desc.set_text, d)
            threading.Thread(target=fetch, daemon=True).start()
        elif not p.description:
            desc.set_text(p.summary or "No description available.")
        box.append(label("About", "title-4"))
        box.append(desc)

        grp = Adw.PreferencesGroup(title="Details")
        rows = [("Source", {"snap": "Snap", "deb": "Debian package (apt)", "flatpak": "Flatpak", "manual": "Manual install (not deb, snap or flatpak)"}[p.kind]),
                ("Version", p.version), ("Installed size", GLib.format_size(p.size) if p.size else ""),
                ("Package name" if p.kind != "manual" else "Launcher", p.pkgid)]
        if p.kind == "snap":
            rows += [("Publisher", p.publisher), ("Channel", p.channel), ("Revision", p.revision)]
        elif p.kind == "flatpak":
            rows += [("Origin", p.channel), ("Installed for", p.section), ("Branch", p.revision)]
        elif p.kind == "manual":
            rows += [("Location", p.location)]
        else:
            rows += [("Section", p.section)]
        for t, v in rows:
            if v:
                r = Adw.ActionRow(title=t, subtitle=GLib.markup_escape_text(str(v)), subtitle_selectable=True)
                r.add_css_class("property")
                grp.add(r)
        if p.homepage:
            r = Adw.ActionRow(title="Website", subtitle=GLib.markup_escape_text(p.homepage), activatable=True)
            r.add_suffix(Gtk.Image(icon_name="adw-external-link-symbolic"))
            r.connect("activated", lambda *_: Gtk.UriLauncher.new(p.homepage).launch(self.win, None, None, None))
            grp.add(r)
        box.append(grp)

        tv.set_content(Gtk.ScrolledWindow(child=Adw.Clamp(maximum_size=760, child=box)))
        self.nav.push(Adw.NavigationPage(title=p.title, child=tv))

    def launch(self, p):
        info = Gio.DesktopAppInfo.new_from_filename(p.desktop)
        if info:
            info.launch([], None)

    # ---- uninstall flow
    def start_uninstall(self, p, button):
        button.set_sensitive(False)

        def work():
            extra, blocked = simulate_deb_removal(p.pkgid) if p.kind == "deb" else ([], None)
            GLib.idle_add(self.confirm, p, extra, blocked, button)
        threading.Thread(target=work, daemon=True).start()

    def confirm(self, p, extra, blocked, button):
        button.set_sensitive(True)
        if blocked:
            d = Adw.AlertDialog(heading="Can't uninstall", body=blocked)
            d.add_response("close", "Close")
            d.present(self.win)
            return False

        d = Adw.AlertDialog(heading=f"Uninstall {p.title}?" if (p.kind != "manual" or p.appdir) else f"Remove {p.title} launcher?")
        if p.kind == "manual":
            d.set_body("These files will be deleted:" if p.appdir else
                       f"Only the launcher entry is removed. The program itself ({p.location}) is left untouched.")
        else:
            d.set_body("This removes the app from your computer." if p.kind in ("snap", "flatpak") else
                       "The package will be removed with apt." + (
                           f"\n\nThis will also remove {len(extra)} package(s) that depend on it:" if extra else ""))
        content = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
        if p.kind == "manual" and p.appdir:
            extra = [t.replace(HOME, "~") for t in p.plan]
        if extra or p.kind == "manual":
            tvw = Gtk.Label(label="\n".join(sorted(extra)) if p.kind != "manual" else "\n".join(extra or [p.desktop.replace(HOME, "~")]), xalign=0, selectable=True)
            tvw.add_css_class("monospace")
            sc = Gtk.ScrolledWindow(child=tvw, min_content_height=60, max_content_height=180, propagate_natural_height=True)
            sc.add_css_class("card")
            content.append(sc)
        purge = Gtk.CheckButton(label="Also delete settings and data" if p.kind != "snap"
                                else "Don't keep a backup snapshot of its data")
        if p.kind == "manual":
            purge.set_label("Also delete its settings and data (" + ", ".join(t.replace(HOME, "~") for t in p.data) + ")")
        if p.kind != "manual" or p.data:
            content.append(purge)
        d.set_extra_child(content)
        d.add_response("cancel", "Cancel")
        d.add_response("remove", "Uninstall" if (not extra or p.kind == "manual") else f"Uninstall {1 + len(extra)} packages")
        d.set_response_appearance("remove", Adw.ResponseAppearance.DESTRUCTIVE)
        d.set_default_response("cancel")
        d.set_close_response("cancel")
        d.connect("response", lambda _, r: r == "remove" and self.do_uninstall(p, purge.get_active()))
        d.present(self.win)
        return False

    def do_uninstall(self, p, purge):
        if p.kind == "manual":
            def work_m():
                ok, msg = remove_manual(p, purge)
                r = subprocess.CompletedProcess([], 0 if ok else 1, "", msg)
                GLib.idle_add(self.done, p, r, busy)
            busy = self.make_busy(p, False)
            threading.Thread(target=work_m, daemon=True).start()
            return
        root = True
        if p.kind == "flatpak":
            root = p.section == "system"
            cmd = ["flatpak", "uninstall", "-y", "--noninteractive", "--system" if root else "--user"] \
                + (["--delete-data"] if purge else []) + [p.pkgid]
        elif p.kind == "snap":
            cmd = ["snap", "remove"] + (["--purge"] if purge else []) + [p.pkgid]
        else:
            cmd = ["apt-get", "purge" if purge else "remove", "-y", p.pkgid]
        busy = self.make_busy(p, root)

        def work():
            r = run_privileged(cmd, root)
            GLib.idle_add(self.done, p, r, busy)
        threading.Thread(target=work, daemon=True).start()

    def make_busy(self, p, root=True):
        busy = Adw.Dialog(title="", content_width=300, can_close=False)
        bx = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=14, margin_top=28, margin_bottom=28,
                     margin_start=24, margin_end=24)
        bx.append(Gtk.Spinner(spinning=True, width_request=32, height_request=32))
        bx.append(Gtk.Label(label=f"Removing {p.title}…" + ("\nApprove the password prompt if it appears." if root else ""),
                            justify=Gtk.Justification.CENTER))
        busy.set_child(bx)
        busy.present(self.win)
        return busy

    def done(self, p, r, busy):
        # Fast removals finish before the dialog has finished opening; closing it then is ignored.
        GLib.timeout_add(500, lambda: (busy.force_close(), self.finish(p, r))[1])
        return False

    def finish(self, p, r):
        if r.returncode == 0:
            self.toasts.add_toast(Adw.Toast(title=f"{p.title} was uninstalled"))
            self.nav.pop_to_page(self.main_page)
            self.reload()
        elif r.returncode in (126, 127):
            self.toasts.add_toast(Adw.Toast(title="Cancelled: authentication was not granted"))
        else:
            tail = "\n".join((r.stderr or r.stdout).strip().splitlines()[-8:])
            d = Adw.AlertDialog(heading="Uninstall failed", body=tail or f"Exit code {r.returncode}")
            d.add_response("close", "Close")
            d.present(self.win)
        return False


if __name__ == "__main__":
    sys.exit(Hub().run(sys.argv))
