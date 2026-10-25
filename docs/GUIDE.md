# Getting started with Depot

This guide walks you through Depot one screen at a time. You don't need to know any Linux commands.

## 1. Open Depot

Open your app menu, type **Depot** and click it. Depot reads everything installed on your computer
and shows your apps as cards. Grey placeholder cards show while it reads; the first time can take
a few seconds, after that it's almost instant.

## 2. Find your way around

The list on the left is the **sidebar**. The number next to each entry tells you how many items it holds.

| Sidebar entry | What you'll find there |
| --- | --- |
| **All apps** | Every app that has an icon in your app menu |
| **Updates** | Apps with a newer version waiting (see step 5) |
| **Deb apps**, **RPM apps** or **Pacman apps** | Apps from your system's own software sources (which one depends on your Linux) |
| **Snap apps** | Apps from the Snap Store |
| **Flatpak apps** | Apps from Flathub, which run in their own sandbox |
| **AppImages** | Apps you downloaded as a single `.AppImage` file, whether or not they're in your app menu |
| **Manual installs** | Apps you downloaded and installed by hand |
| **Broken launchers** | Menu entries whose app is already gone (see step 6) |
| **Advanced** | Click to show libraries, command-line tools and runtimes. You rarely need these. |

Entries with nothing in them are hidden. If your window is narrow, the sidebar tucks away; click the
sidebar button in the top-left corner to show it.

**Searching:** click the search box at the top, or press **Ctrl+F**, and type part of a name. If
nothing matches, Depot says so and offers **Clear search**.
**Sorting:** the menu next to the search box sorts by **Name** or by **Size (largest first)**. Sorting
by size is the quickest way to find what's using your disk space.

### What the coloured badges mean

Each card has a badge saying how the app was installed. Hover over a badge for a short explanation.

- **Deb**, **RPM** or **Pacman**: installed from your system's software sources (with apt, dnf or
  pacman).
- **AUR**: an Arch package built from the AUR or another source outside your repositories. Your
  normal updates don't include it.
- **Snap**: installed from the Snap Store. Snaps update themselves.
- **Flatpak**: installed with Flatpak, usually from Flathub.
- **AppImage**: an app packed into one downloaded file. Removing it moves the file to the Trash.
- **Manual**: installed by hand, for example from a downloaded archive or script.
- **Broken**: the launcher is still in your menu, but its app is gone.

## 3. Look at an app

Click any card to open its page. You'll see:

- **Open** starts the app.
- **Uninstall** removes it (step 4).
- **About** describes what the app does.
- **Details** lists where it came from, its version and size, its package name and, for
  Flatpaks, whether it's installed for everyone or only for you.

Click the back arrow at the top-left to return to the list.

**Installed more than once?** Sometimes the same app is on your computer twice, for example as a
Deb and as a Flatpak. Depot shows it as one card with a badge for each way it's installed. Its page
has an **Installed 2 ways** section listing every copy; click **Show** next to one to see its details
or uninstall just that copy. You usually only need one.

If the Uninstall button is greyed out, hover over it to see why. Depot won't remove things your
computer needs to work, such as the core of the system or the runtimes that Flatpak apps share.
(Runtimes no app uses any more can be cleared out in one go: see section 7.)

## 4. Uninstall an app

1. Open the app's page and click **Uninstall**.
2. Depot works out what would happen and shows it to you **before anything changes**:
   - Other packages that need this app are listed. They have to go with it.
   - **"Also remove N packages that nothing else needs any more"** lists leftovers from earlier
     installs. It's ticked by default to free up space. Untick it if you see a tool you still use
     in that list.
   - **"Also delete its system-wide settings files"** removes the app's configuration files from
     the system folders. Your own files in your home folder are never touched.
   - For Snaps, **"Don't keep a backup snapshot of its data"** skips the automatic backup.
3. Click **Uninstall**. If your computer asks for your password, type it. That's your system
   checking it's really you, not Depot.

For apps installed by hand, Depot lists the files it found and the confirm button says **Move to
Trash**. The files go to your Trash, so if you change your mind you can open the Trash and restore
them. If Depot only found the app's menu entry, the button on its page says **Remove launcher** and
the program itself is left alone.

If Depot says **"Your system needs this package to work"**, it has refused to remove something
that would break your computer. That's on purpose.

### What you see while it works

After you press **Uninstall**, a progress window opens and tells you, step by step:

1. **Waiting for your password.** A password window from your system opens. Type your password
   there.
2. **Password accepted** appears with a green tick once it worked. If you close the password window
   instead, Depot says **Nothing was changed**.
3. **Waiting for another app to finish** means the Software app or automatic updates are busy.
   Depot carries on by itself as soon as they're done.
4. At the end it says how it went: **Firefox was uninstalled** (and how much space was freed), or
   what didn't work and why, with **Show details**, **Copy details** and **Try again**.

### Removing several apps at once

1. Press **Select** at the top, or hold **Ctrl** and click a card. On a
   touchscreen, press and hold a card.
2. Tick every app you want to remove. Clicking a card now ticks it instead of opening it. **Select
   all** (or **Ctrl+A**) ticks every app shown, which works well together with search or a
   category. Apps Depot can't remove can't be ticked; it says why.
3. The bar at the bottom says how many you've ticked and how much space they use. Press
   **Uninstall**.
4. Depot shows everything that will change, grouped by where each app came from, and removes them
   in as few steps as possible (usually one password prompt for system apps). The progress window
   ticks off each app as it goes.

Press **Cancel** or **Esc** to stop selecting.

## 5. Keep your apps up to date

Click **Updates** in the sidebar. Depot asks every software source for newer versions, which takes a
few seconds.

- **Apps** lists each app with its current and new version, for example `1.97.56 → 1.97.57`.
  Click **Update** next to one app to update only that app.
- **System updates** groups libraries, drivers and runtimes into one entry. Click the arrow to see
  what's inside, or click **Update** to install them all.
- A **Security** badge marks fixes for security problems. Install these soon. If you untick one,
  Depot reminds you at the top of the page.
- Every update has a **tick box**, all ticked to start with. Untick the ones you want to skip;
  the button in the top bar changes from **Update all** to **Update 5 selected**. **Select none** /
  **Select all** above the list ticks or unticks everything. Updates you unticked stay unticked
  when the list refreshes.

The same progress window as for uninstalling shows how far along it is, when your password was
accepted, and which groups of updates (system packages, Snaps, Flatpaks) are done.

Very rarely an update needs to remove another package. Depot then stops, tells you which one, and
lets you choose **Update anyway** or **Cancel**.

The number next to **Updates** in the sidebar refreshes whenever Depot starts. The page says when it
last checked. Click the **refresh** button (the circular arrow), or press **F5**, to check again right
away. If Depot couldn't reach any software source, it says so and asks you to check your internet
connection.

## 6. Clean up broken launchers

When an app is removed but its menu entry stays behind, you get an icon in your app menu that does
nothing. Depot lists these under **Broken launchers**. Open one and click **Remove leftover
launcher** to move it to the Trash.

## 7. Free up space from Flatpak runtimes

Flatpak apps share building blocks called **runtimes**, and a runtime can take a gigabyte or more.
When you uninstall the last app that needed one, the runtime stays behind. Click **Advanced**, then
**Runtimes & bases**: if any runtimes aren't used by an app any more, a bar at the top says how many
and how much space they take. Click **Clean up**, check the list, and click **Remove**.

Runtimes you installed yourself on purpose (Flatpak calls these "pinned") are never offered. If an
app needs a removed runtime again later, Flatpak downloads it when you install that app.

## 8. When something goes wrong

| What you see | What to do |
| --- | --- |
| "Nothing was changed" | The password window was closed or the password was wrong. Press **Try again**. |
| "Another app is installing or updating software right now" | The Software app or automatic updates are busy. Wait a minute, then press **Try again**. |
| "The app is still open" | Close the app (Snaps refuse to update or remove a running app), then try again. |
| "Some of them couldn't be removed" | The progress window lists what worked and what didn't. **Show details** says why; **Try again** retries only what's left. |
| "Couldn't read installed software" | Press **Try again**. If it keeps happening, please report it (below). |
| "Couldn't check everything" on the Updates page | Check your internet connection, then click the refresh button. |
| An app is missing or in the wrong place | Click the refresh button on the main page. If it's still wrong, please report it (below). |

## 9. Menu, shortcuts and About

The menu button (three lines, top right) has **Select Apps**, **Keyboard Shortcuts** (also
**Ctrl+?**), **Privacy**, **Report a Problem** and **About Depot**. The About window shows the
version, what's new, the licence and legal notices, and a **Troubleshooting** page you can copy
into a bug report.

Handy keys: **Ctrl+F** search, **F5** refresh, **Esc** go back or stop selecting, **Ctrl+A** select
all apps shown (while selecting), **Ctrl+Q** quit.

### Reporting a problem

Open a **Terminal** and run:

```sh
depot --list-only > depot-report.json
```

This saves a list of everything installed to `depot-report.json` in your home folder. Attach it to a
new issue at <https://github.com/kimzam30/Nhako-Depot/issues> and describe what you expected to
see. The file lists package and app names and the folders they're installed in, which can include
your username. It contains no personal files or their contents.
