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
cd desktop/src-tauri && cargo test -p process -p steam --lib
```
```bash
powershell -ExecutionPolicy Bypass -File scripts/verify-client-only.ps1
```

Back up `%APPDATA%\drop\drop.db`. Note the installer's SHA-256 so you can prove
later which build was tested.

If you are going to run the Steam tests, also copy
`<Steam>\userdata\<account>\config\shortcuts.vdf` somewhere safe first. Drop
keeps its own rolling backups, but your own copy costs nothing.

Record for each test: **PASS / FAIL**, date, build SHA-256, and for a failure
the relevant lines of `%APPDATA%\drop\drop.log` plus any
`%APPDATA%\drop\crash-*.log`.

---

## Automated

| ID | Covers | Command |
|---|---|---|
| A-1 | ZC-003 Windows tokenising, quoting, round-trip, coalescing (15 tests) | `cargo test -p process --lib` |
| A-2 | ZC-004/005/006 Steam app-id stability, dedup, backups, preserved playtime, artwork slots (19 tests) | `cargo test -p steam --lib` |
| A-3 | Client-only rule, both directions | `scripts/verify-client-only.ps1` |
| A-4 | Client compiles | `cargo check` (in `desktop/src-tauri`) |

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

## TEST 7 — Add to Steam *(ZC-004)*

> Steam may be running. Drop closes it, writes, and starts it again — that is
> the intended flow and is itself worth testing (7d).

1. Install a game via Drop.
2. Open the game's options → **Steam** tab.
3. Confirm the panel lists your Steam account and the launcher, and shows the
   resolved target path under "Steam will run:".
4. Click **Add to Steam**.

**Expected.** The game appears in the Steam library as a non-Steam shortcut,
under a **Drop** category.

**7b — no duplicate.** Click **Update shortcut**. Exactly one entry.

**7c — other shortcuts survive.** If you already had non-Steam shortcuts (this
machine has "Cyberpunk 2077 (GOG)"), they must still be present and unchanged.

**7d — Steam is closed and restarted, not killed.** With Steam **running**,
click **Add to Steam**. Steam should close cleanly and come back on its own, and
the panel should say so. Confirm Steam did not crash-report on restart.

**7e — Steam refuses to close during a game.** Launch any game through Steam,
then click **Add to Steam**. Drop must report *"Steam would not close…"* and
must **not** write. Quit the game and retry — it should then work. This is the
case where forcing would corrupt Steam's config, so a failure here is serious.

**7f — genuine licences untouched.** If you own the same game on Steam for
real, that entry is unaffected. (By construction: `shortcuts.vdf` holds only
non-Steam shortcuts; real licences live in `steamapps/appmanifest_*.acf`, which
Drop never opens.)

**Recovery.** Backups are kept at
`<Steam>\userdata\<account>\config\shortcuts.vdf.drop-backup-<timestamp>`
(5 rolling). Restore one by renaming it over `shortcuts.vdf` with Steam closed.

---

## TEST 8 — Steam shows the game correctly *(ZC-004)*

In Steam, right-click the shortcut → Properties.

**Expected.** Correct name; **Target** is the game executable — *not*
`drop-app.exe`, *not* `cmd.exe`, *not* `powershell.exe`; correct **Start in**
directory; correct launch arguments.

**Also check a game whose executable has a space** (Graveyard Keeper): the
target must be the full `...\Graveyard Keeper.exe`, quoted.

---

## TEST 9 — Steam artwork *(ZC-005)*

**9a — no API key (the important one).** With no SteamGridDB key saved (use
**Forget** if one is stored), add a game to Steam.

**Expected.** The library tile shows Drop's own cover art, not a grey box. The
panel lists which slots were filled. **The feature must work without a key** —
this is the path most members will be on.

**9b — with a SteamGridDB key.** Get a free key from
`steamgriddb.com/profile/preferences/api`, paste it into the panel, **Save**,
then **Update shortcut** on a game.

**Expected.** Proper Steam artwork: vertical capsule, wide capsule, hero banner,
and where available a transparent logo and an icon.

**9c — the key never comes back.** After saving, the field shows only *"A key is
saved"* — the key itself is never returned to the UI. Confirm it is stored at
`%APPDATA%\drop\zougcloud\steamgriddb.key` and **not** anywhere in `drop.db`.

**9d — files land in the right place.** Check
`<Steam>\userdata\<account>\config\grid\` for `<appid>.png`, `<appid>p.png`,
`<appid>_hero.png`, `<appid>_logo.png`, `<appid>_icon.png`.

**9e — no stale slot.** Add with no key (Drop art), then save a key and update.
Each slot must have exactly **one** file — no `123p.jpg` left beside a
`123p.png`, which Steam would render instead.

**9f — removal cleans up.** **Remove from Steam**, then check the `grid` folder:
that appid's files are gone, and other games' artwork is untouched.

---

## TEST 10 — Steam Play launches the real game *(ZC-004)*

Press **Play** in Steam.

**Expected.** The game starts directly. Playtime accrues against the game.
`drop-app.exe` is **not** launched — otherwise Steam would be timing the
launcher instead of the game. Check the Task Manager while the game runs: no
`drop-app.exe` spawned by Steam, no stray `cmd.exe`.

**Also check.** The Steam overlay (Shift+Tab) opens over the game.

---

## TEST 11 — A Drop update preserves the Steam shortcut *(ZC-006)*

1. Add a game to Steam and play it briefly so Steam records some playtime.
2. Note the shortcut's AppID (visible in the shortcut's Steam URL, or in the
   `grid` filenames).
3. Update the game through Drop to a new version.
4. Reopen the game's **Steam** tab in Drop.

**Expected.** The shortcut still exists, still has its playtime, and Drop still
reports it as added. Nothing is removed automatically.

**If the install path changed**, click **Update shortcut** and confirm the
**AppID is unchanged** — Steam keys playtime on that id and names artwork files
after it, so a new id would silently orphan both. This is the single most
important assertion in this test.

---

## TEST 12 — Multiple Steam accounts *(ZC-004)*

Only applicable on a machine with more than one Steam account (this build
machine has two).

**Expected.** The account selector lists them by persona name, defaults to the
most recently used, and switching it re-reads that account's shortcuts — a game
added for one account is not reported as added for the other.

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
