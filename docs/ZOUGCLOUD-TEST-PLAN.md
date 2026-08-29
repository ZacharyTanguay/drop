# ZougCloud Desktop — test plan

Run before distributing any installer to members.

**Environment**

| | |
|---|---|
| Server | `ghcr.io/drop-oss/drop:0.4.0-rc-5` — **must not be upgraded for these tests** |
| Client | the ZougCloud build under test |
| OS | Windows 10/11 x64 |
| Reference title | Graveyard Keeper (Unity, executable `Graveyard Keeper.exe`) |

**Before starting**

```bash
cd desktop/src-tauri && cargo test -p process --lib
```
```bash
powershell -ExecutionPolicy Bypass -File scripts/verify-client-only.ps1
```

Back up `%APPDATA%\drop\drop.db`. Note the installer's SHA-256 so you can prove
later which build was tested.

Record for each test: **PASS / FAIL**, date, build SHA-256, and for a failure
the relevant lines of `%APPDATA%\drop\drop.log` plus any
`%APPDATA%\drop\crash-*.log`.

---

## Automated

| ID | Covers | Command |
|---|---|---|
| A-1 | ZC-003 Windows tokenising, quoting, round-trip, coalescing (15 tests) | `cargo test -p process --lib` |
| A-2 | Client-only rule, both directions | `scripts/verify-client-only.ps1` |
| A-3 | Client compiles | `cargo check` (in `desktop/src-tauri`) |

> Do **not** use `cargo check --workspace`: `cloud_saves` does not compile
> upstream and is not a dependency of `drop-app`, so `tauri build` never
> reaches it either.

---

## TEST 1 — Open → Close → Tray → Open *(ZC-001)*

1. Launch Drop, let it connect.
2. Close the window with the **X**.
3. Confirm the window disappears and the tray icon remains.
4. Confirm `drop-app.exe` is **still running** in the Task Manager — this is the
   intended tray behaviour, not the bug.
5. Tray icon → **Open**.

**Expected.** The window reappears. **No crash.** No new
`%APPDATA%\drop\crash-*.log`.

**Regression guarded.** Pre-fix this panicked every time with
`Failed to get webview`.

Repeat **three times** in a row.

---

## TEST 2 — Open → Close → relaunch the shortcut *(ZC-001, the core fix)*

1. Launch Drop, let it connect.
2. Close with the **X** (now hidden in the tray).
3. Double-click the Drop desktop/Start-menu shortcut again.

**Expected.** The existing window comes back to the front, focused and
unminimised. **Exactly one** `drop-app.exe` in the Task Manager — never two.

**Regression guarded.** Pre-fix, nothing happened at all; users killed the
process manually.

Repeat **three times**. Also try it with the window minimised rather than
hidden.

---

## TEST 3 — Tray → Quit → process gone *(ZC-001)*

1. Launch Drop.
2. Tray icon → **Quit**.
3. Watch the Task Manager.

**Expected.** `drop-app.exe` disappears within a couple of seconds. The tray
icon disappears. Relaunching Drop afterwards works normally, with no Task
Manager intervention.

---

## TEST 4 — Unquoted executable with a space *(ZC-003)*

In Drop Admin, set the Graveyard Keeper launch command to exactly:

```
Graveyard Keeper.exe
```

(no quotes — the UI shows a fixed `(install_dir)/` prefix, which is only a
label, not part of the stored value)

Launch from the Desktop.

**Expected.** The game starts. `%APPDATA%\drop\drop.log` shows
`coalesced unquoted command 'Graveyard' into 'Graveyard Keeper.exe'`.

**Regression guarded.** Pre-fix:
`'Graveyard' is not recognized as an internal or external command`.

---

## TEST 5 — Quoted executable with a space *(ZC-003)*

Same, with the command set to:

```
"Graveyard Keeper.exe"
```

**Expected.** The game starts. The log shows the `Direct` handler (no `cmd.exe`,
no `pwsh`).

**Also check** — these must all still work:

| Command | Expected |
|---|---|
| `Game.exe` | launches |
| `Game.exe --windowed` | launches, argument passed through |
| `"Game With Spaces.exe" --windowed` | launches, argument passed through |

---

## TEST 6 — Update detected without restarting *(ZC-002)*

1. Install a game at version N with "Latest".
2. In the game's options, turn **Enable update checks ON**. *(Required — games
   without it are never polled. Default is off.)*
3. Leave the Desktop **open** on the library.
4. Import version N+1 on the server.
5. Wait up to **6 minutes** (poll interval is 5).

**Expected.** The **Update** button appears **without restarting the Desktop**.

**Regression guarded.** Pre-fix it stayed "Up to date" until a full restart.

**Also check.** During a quiet period (no new version), the UI does not flicker
or re-render on every poll — the event only fires on an actual transition.

---

## TEST 7 — Add to Steam *(ZC-004 — not yet implemented)*

1. Install a game via Drop.
2. Use **Add to Steam**.
3. Restart Steam.

**Expected.** The game appears in the Steam library as a non-Steam shortcut.
Adding twice does not create a duplicate. No existing genuine Steam licence for
the same title is modified.

---

## TEST 8 — Steam shows the game correctly *(ZC-004)*

**Expected.** Correct name, correct target executable, correct working
directory, correct arguments.

---

## TEST 9 — Steam artwork *(ZC-005)*

**Expected.** Grid/capsule, hero, logo and icon render after a Steam restart.
Verify **both** paths: with a SteamGridDB API key configured, and with none
(fallback to Drop's own images). The feature must work without a key.

---

## TEST 10 — Steam Play launches the real game *(ZC-004)*

Press **Play** in Steam.

**Expected.** The game starts directly. Playtime accrues against the game.
`drop-app.exe` is **not** launched — otherwise Steam would be timing the
launcher instead of the game.

---

## TEST 11 — A Drop update preserves the Steam shortcut *(ZC-006)*

1. Add a game to Steam.
2. Update it through Drop to a new version.

**Expected.** Shortcut, artwork, identity and Steam playtime all survive. The
shortcut is only rewritten if the install path actually changed. It is **never**
removed automatically.

---

## Installer checks

| ID | Check | Expected |
|---|---|---|
| I-1 | Install over a previous ZougCloud build | Upgrades in place; no duplicate entry in Apps & Features |
| I-2 | `%APPDATA%\drop\games` after upgrade | Untouched; installed games still listed |
| I-3 | `%APPDATA%\drop\drop.db` after upgrade | Preserved; config and library intact |
| I-4 | SHA-256 of the shipped `.exe` | Matches `SHA256SUMS.txt` |
| I-5 | `BUILD-INFO.txt` | Records upstream base commit **and** ZougCloud commit |

> The Tauri NSIS uninstaller only ever removes `%APPDATA%\org.droposs.client`
> and `%LOCALAPPDATA%\org.droposs.client`, and only when the "delete application
> data" box is ticked **and** it is not an upgrade. Drop's real data directory
> (`%APPDATA%\drop`) is never a target — but verify I-2 and I-3 anyway on every
> release.
