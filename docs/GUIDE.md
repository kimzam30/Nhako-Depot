# Getting started with Depot

This guide walks you through Depot one screen at a time. You don't need to know any Linux commands.

## 1. Open Depot

Open your app menu, type **Depot** and click it. Depot reads everything installed on your computer
in a fraction of a second and shows your apps as cards.

## 2. Find your way around

The list on the left is the **sidebar**. The number next to each entry tells you how many items it holds.

| Sidebar entry | What you'll find there |
| --- | --- |
| **All apps** | Every app that has an icon in your app menu |
| **Updates** | Apps with a newer version waiting (see step 5) |
| **Deb apps** | Apps from your system's own software sources |
| **Snap apps** | Apps from the Snap Store |
| **Flatpak apps** | Apps from Flathub, which run in their own sandbox |
| **Manual installs** | Apps you downloaded and installed by hand |
| **Broken launchers** | Menu entries whose app is already gone (see step 6) |
| **Advanced** | Click to show libraries, command-line tools and runtimes. You rarely need these. |

Entries with nothing in them are hidden. If your window is narrow, the sidebar tucks away; click the
sidebar button in the top-left corner to show it.

**Searching:** click the search box at the top, or press **Ctrl+F**, and type part of a name.
**Sorting:** the menu next to the search box sorts by **Name** or by **Size (largest first)**. Sorting
by size is the quickest way to find what's using your disk space.

### What the coloured badges mean

Each card has a badge saying how the app was installed. Hover over a badge for a short explanation.

- **Deb**: installed from your system's software sources with apt.
- **Snap**: installed from the Snap Store. Snaps update themselves.
- **Flatpak**: installed with Flatpak, usually from Flathub.
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

If the Uninstall button is greyed out, hover over it to see why. Depot won't remove things your
computer needs to work, such as the core of the system or the runtimes that Flatpak apps share.

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

## 5. Keep your apps up to date

Click **Updates** in the sidebar. Depot asks every software source for newer versions, which takes a
few seconds.

- **Apps** lists each app with its current and new version, for example `1.97.56 → 1.97.57`.
  Click **Update** next to one app to update only that app.
- **System updates** groups libraries, drivers and runtimes into one entry. Click the arrow to see
  what's inside, or click **Update** to install them all.
- A **Security** badge marks fixes for security problems. Install these soon.
- **Update all** in the top bar installs everything at once.

A progress bar shows how far along it is. You may be asked for your password.

Very rarely an update needs to remove another package. Depot then stops, tells you which one, and
lets you choose **Update anyway** or **Cancel**.

The number next to **Updates** in the sidebar refreshes whenever Depot starts. Click the **refresh**
button (the circular arrow) on the Updates page to check again right away.

## 6. Clean up broken launchers

When an app is removed but its menu entry stays behind, you get an icon in your app menu that does
nothing. Depot lists these under **Broken launchers**. Open one and click **Remove leftover
launcher** to move it to the Trash.

## 7. When something goes wrong

| What you see | What to do |
| --- | --- |
| "Cancelled: authentication was not granted" | The password window was closed or the password was wrong. Try again. |
| "Uninstall failed" or "Update failed" | Another program may be installing software at the same time, such as the Software app or automatic updates. Wait a minute and try again. |
| "Couldn't check everything" on the Updates page | Check your internet connection, then click the refresh button. |
| An app is missing or in the wrong place | Click the refresh button on the main page. If it's still wrong, please report it (below). |

### Reporting a problem

Open a **Terminal** and run:

```sh
depot --list-only > depot-report.json
```

This saves a list of everything installed to `depot-report.json` in your home folder. Attach it to a
new issue at <https://github.com/kimzam30/Nhako-Depot/issues> and describe what you expected to
see. The file lists package and app names and the folders they're installed in, which can include
your username. It contains no personal files or their contents.
