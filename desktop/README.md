# desktop/

Tauri v2 tray / menu-bar app for macOS and Windows. Uses `kitedrop-core` directly (docs/api.md).

| Path | What |
|---|---|
| `src-tauri/src/lib.rs` | app: node start, tray menu, windows, global shortcut, drop window, commands for the UI |
| `src-tauri/src/platform.rs` | identity in Keychain / Credential Manager, device name, sleep assertion, Windows network profile |
| `src-tauri/windows/hooks.nsh` | installer: firewall rule (Private profile only) and the classic "Send to" entry |
| `ui/` | static HTML/JS (no build step); peer-supplied text is set with `textContent` only |

Build: `cargo tauri build` in `src-tauri/` (macOS `.app`/`.dmg`; Windows NSIS installer, see decisions D30).

Development run that cannot touch the installed app (debug builds only):

```
KITEDROP_DEV_DIR=/tmp/kd-dev cargo run -p kitedrop-desktop -- --pair 'kitedrop://pair?...'
```

keeps data, downloads and a plain key file in that folder, skips the Keychain and login items, and `--pair`
acts on a pairing link at start (use `kitedrop daemon` from `cli/` as the other device).
