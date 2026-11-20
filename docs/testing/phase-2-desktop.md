# Phase 2 desktop test: Mac ↔ Windows (owner)

Exit criterion (plan Phase 2): all flows work between two desktops on Mac and Windows.

Build on the Mac: `scripts/build-desktop.sh mac` and `scripts/build-desktop.sh windows`.

| Artifact | Path |
|---|---|
| Mac | `target/release/bundle/macos/kitedrop-macos.zip` (ad-hoc signed) |
| Windows | `target/x86_64-pc-windows-msvc/release/bundle/nsis/kitedrop_0.1.0_x64-setup.exe` (unsigned) |

## Install

1. Mac: unzip, move `kitedrop.app` to Applications, right-click → Open (ad-hoc signed). Allow the Keychain
   item and "Local Network" when asked. The kite icon appears in the menu bar.
2. Windows: copy the setup file over, run it. SmartScreen: More info → Run anyway; UAC: Yes (needed for the
   firewall rule). The kite icon appears in the tray (may be under the ^ arrow).

## Flows

| # | Flow | Expected |
|---|---|---|
| 1 | Mac: tray → Add a device → Copy link; send the link to the PC (chat/e-mail) and paste it there: Windows tray → Add a device → paste → Pair, within 60 s | both list each other, green dot |
| 2 | Mac: copy a sentence, ⌘⇧K, pick the PC | PC notification, text in the PC clipboard (Ctrl+V) |
| 3 | PC: copy a link, Ctrl+Shift+K, pick the Mac | Mac notification, link in the clipboard |
| 4 | Mac: tray → Drop window, drop a photo, pick the PC | file in `Downloads\kitedrop` on the PC |
| 5 | PC: Explorer, right-click a file → Send to → kitedrop, pick the Mac | file in `~/Downloads/kitedrop` |
| 6 | PC: tray → Quit kitedrop. Mac: send a text to the PC | Mac main window: "Waiting", reason offline |
| 7 | PC: start kitedrop again | Mac shows "Delivered" within about a minute |
| 8 | PC: send a ~1 GB file to the Mac | arrives, same size |
| 9 | PC: set the network to Public (Settings → Network → properties), open the kitedrop window | warning about the Public profile |
| 10 | Restart the PC, do not open kitedrop | it starts by itself (tray icon); a send from the Mac arrives |

Report per row: works / does not (with what you saw).
