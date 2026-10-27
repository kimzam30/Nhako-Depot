# Screen reader checklist (Orca)

Run this before a release, with the mouse put away: everything here is done with the keyboard
and checked by ear. It takes about 15 minutes. Write down anything Orca says wrong, says twice,
or doesn't say at all.

## Before you start

1. Turn Orca on: **Super+Alt+S** (or Settings → Accessibility → Screen Reader).
2. Start Depot from the app menu, not a terminal, so it runs the way people use it.
3. Use **Tab** / **Shift+Tab** to move between parts, **arrow keys** inside lists and the grid,
   **Enter** or **Space** to press. **Orca+Space** opens Orca's own settings if speech is too fast.

"Should hear" gives the gist; the exact wording may differ a little.

## 1. Main window

| Do | Should hear |
| --- | --- |
| Open Depot | "Depot", then the search box |
| Tab to the sidebar list, arrow down through it | Each category with its count: "Flatpak apps, 15" |
| Arrow to **Advanced** | "Advanced", "collapsed" |
| Press Enter on Advanced | "expanded"; the advanced categories (Developer tools, …) now follow |
| Arrow to **Updates** and back to **All apps** | The page changes; nothing is read twice |

## 2. The grid

| Do | Should hear |
| --- | --- |
| Tab into the grid, arrow through a few cards | Name, how it was installed and size: "Firefox, Flatpak, 300 MB" |
| Type a word in the search box (Ctrl+F first) | After a moment: "3 apps found", or "No results" |
| Clear the search | The grid comes back; nothing is announced |

## 3. An app's page

| Do | Should hear |
| --- | --- |
| Enter on a card | The app's name as the page title |
| Tab through the page | "Open", "Uninstall", then the details rows |
| On an app installed more than one way: Tab to the copies | "Show the Snap copy" (not just "Show") |
| Press Open | "Opening" |
| Escape | Back on the grid (write down where focus lands) |

## 4. Selecting several apps

| Do | Should hear |
| --- | --- |
| Tab to **Select** and press it | "Selecting apps. Tick the ones you want to uninstall." |
| Arrow to a card, press Space | "1 selected · 12 MB", and the card is read as "selected" |
| Tick a second card | "2 selected · …" |
| Untick both | "Nothing selected" |
| Tab to the bottom bar | "Select all", the count, "Uninstall" |
| Press Escape | "Stopped selecting" |

## 5. Uninstalling (pick something you can reinstall, or Cancel at the end)

| Do | Should hear |
| --- | --- |
| On an app's page press Uninstall | "Checking what would change", then the dialog's heading and text |
| Tab through the dialog | Every tick box by its full text; Cancel; Uninstall |
| Press Uninstall (or Cancel here to stop) | "Waiting for your password…" when it asks for one |
| Type the password | "Password accepted" |
| With several apps: wait | Each one as it finishes: "Firefox: Removed" |
| At the end | The result: "Firefox was uninstalled. About 300 MB freed." |
| | The result **stays open** until you close it (it would close by itself without Orca) |
| On a result with details: Tab to "Copy details", press it | "Copied" |
| Start an uninstall and press Cancel while it runs | "Cancelling" |

## 6. Updates

| Do | Should hear |
| --- | --- |
| Go to Updates | After the check: "N updates available" or "Everything is up to date" |
| Tab through the list | Each tick box by name: "Include Firefox in the next update", checked |
| Tab to a row's button | "Update Firefox" (not just "Update") |
| On System updates | "Include all system updates", then "Install the ticked system updates" |
| Untick a security fix | The note "You've unticked a security fix" is reachable with Tab |

## 7. Banners and the rest

| Do | Should hear |
| --- | --- |
| Advanced → Runtimes & bases (if it offers a clean-up) | The banner's text when it appears: "1 shared runtime isn't used…" |
| Main menu (F10): About Depot | The About window, and back with Escape |
| Ctrl+? | The keyboard shortcuts window |

## Known limits

- The result window stays open only when GNOME's screen reader setting is on. On other desktops
  it still closes by itself after 2.5 seconds on success.
- Nothing is announced on systems without a screen-reader connection (`GTK_A11Y=none`): that's on
  purpose, as GTK 4.14 would crash otherwise.
