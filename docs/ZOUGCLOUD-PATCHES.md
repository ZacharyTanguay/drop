# ZougCloud patch inventory

Every divergence from upstream, one entry each. Keep this in step with:

```bash
git grep -n "ZOUGCLOUD("
```

**State legend**

| State | Meaning |
|---|---|
| `custom` | Ours, still required. |
| `upstream fixed` | Upstream now fixes this. The patch should be removed at the next sync. |
| `removable` | No longer needed for another reason (feature dropped, refactored away). |
| `needs review` | Conflicted or behaviourally affected by an upstream change. Re-verify before shipping. |

**Baseline:** upstream `3e378636579c67aae5f7a0026b03ea6f88243f9e` (`develop`, 2026-08-21).
**Target server:** `ghcr.io/drop-oss/drop:0.4.0-rc-5` (commit `6f747186`).

---

## Summary

| ID | Name | State | ZougCloud commit |
|---|---|---|---|
| ZC-001 | Windows tray / single-instance reopen | `custom` | `71eb89ec` |
| ZC-002 | Game update state reaches the UI | `custom` | `4e0c1be2` |
| ZC-003 | Windows launch-command parsing | `custom` | `0d513567` |
| ZC-004 | Steam shortcut integration | `custom` | `415daa63`, `c412152f`, `21a753a4` |
| ZC-005 | Steam artwork | `custom` | `3ea8b514`, `fed5ac6e` |
| ZC-006 | Steam shortcut management | `custom` (in ZC-004) | `415daa63` |
| — | Client-only guard (infrastructure) | `custom` | `469c7dbb` |
| — | Build versioning and packaging | `custom` | `d97a0547`, `f9ae1a98`, `971c22d5` |

---

## ZC-001 — Windows tray / single-instance reopen

**Problem solved.** With Drop hidden in the tray, launching the shortcut again
did nothing at all. Users concluded Drop was broken and killed it from the Task
Manager. Two distinct defects fed this:

1. The tray *Open* item called
   `app.webview_windows().get("frontend").expect("Failed to get webview")`.
   The window is built as a `Window` labelled `"main"` with a **child webview**
   `"frontend"` (`lib.rs`, multiwebview API), and `webview_windows()` only
   returns `WebviewWindow`s — so the map is always empty and the `expect()`
   panicked every time. Confirmed in the field: three
   `%APPDATA%\drop\crash-*.log` files, all
   `panicked at src\lib.rs:374:38: Failed to get webview`.
2. The `tauri_plugin_single_instance` callback was **empty**, so a second launch
   handed over its argv, exited, and the running instance never resurfaced.

**Files.** `desktop/src-tauri/src/lib.rs`

**Why upstream is not enough.** Upstream `12c81eb1` fixes defect (1) —
`get_window("main")` instead of the webview lookup — and this branch inherits
it. Defect (2) is **still present at `3e378636`**: the callback remains an empty
closure with only a comment in it. Our patch covers (2) only.

**Dependencies.** None. Requires the upstream `12c81eb1` fix to be present for
the tray *Open* half; it is, in our base.

**Tests.** Manual TEST 1–3 in `docs/ZOUGCLOUD-TEST-PLAN.md`. Not unit-testable:
the single-instance callback needs a real second process and a live Tauri
runtime.

**ZougCloud commit.** `71eb89ec`

**Upstream reference.** `12c81eb1` — "Fixing linux bug where client would never
actually quit" (#469). Partial overlap only.

**State.** `custom` — watch for upstream filling in the single-instance
callback; the tray half is already theirs.

---

## ZC-002 — Game update state reaches the UI

**Problem solved.** A game installed at version N kept showing "Up to date"
after N+1 was published, until the whole client was restarted. Toggling
"Enable update checks" changed nothing visible.

Three compounding causes:

1. `updates.rs` polled every **30 minutes** when online.
2. When it found a new version it wrote `update_available` into the local
   database and **stopped there** — it never called `push_game_update`, the only
   emitter of `update_game/{id}`.
3. The frontend keeps every game in a module-level registry
   (`desktop/main/composables/game.ts`) that is populated once by `fetch_game`
   and only ever refreshed by that event. No manual refresh exists anywhere in
   the UI.

**Files.** `desktop/src-tauri/src/updates.rs`

**Why upstream is not enough.** `updates.rs` is **byte-identical** between
`v0.4.0-rc-5` and `3e378636`. Upstream has not touched this.

**Dependencies.** Uses existing `games::library::push_game_update` and
`games::state::GameStatusManager`. No new endpoint, no change to any request
shape — fully client-side.

**Design notes.**
- The event fires only on an actual transition of `update_available`, so a poll
  that finds nothing new stays silent instead of waking the UI every cycle.
- `push_game_update` panics if an `Installed` status is pushed without version
  information, so the installed `GameVersion` is passed explicitly.
- `GameStatusManager::fetch_state` is used to build the payload so a running
  game keeps its transient status instead of being reported as merely installed.
- Poll interval 30 → 5 minutes. Only games with `enable_updates` are polled
  (default `false`), so the added server load is one
  `fetch_game_version_options` per opted-in installed game per 5 minutes.

**Tests.** Manual TEST 6 in the test plan. A unit test would need a live server
and Tauri event loop.

**ZougCloud commit.** `4e0c1be2`

**Upstream reference.** None known.

**State.** `custom`

---

## ZC-003 — Windows launch-command parsing

**Problem solved.** A game whose executable contains a space
(`Graveyard Keeper.exe`) refused to launch:

```
'Graveyard' is not recognized as an internal or external command
```

Renaming is not an option — Unity requires `Foo.exe` to sit beside `Foo_Data/`.

**Root cause.** `ParsedCommand::parse` tokenises with `shell_words`, i.e. POSIX
rules, on every platform. On Windows this is wrong twice over:

- backslash is a POSIX escape character, so `C:\Users\Zack` collapses to
  `C:UsersZack`;
- `shell_words::join` quotes with **single** quotes, which `cmd.exe` treats as
  part of the filename.

And with `Graveyard Keeper.exe` split into two tokens, the command becomes
`Graveyard` — no extension — so `windows_launch_command` falls through its
extension sniffing to the `cmd.exe` strategy, producing the error above.

**Files.**
- `desktop/src-tauri/process/src/parser.rs`
- `desktop/src-tauri/process/src/process_handlers.rs`
- `desktop/src-tauri/process/Cargo.toml` (dev-dependency `tempfile`)

**Why upstream is not enough.** `parser.rs`, `process_handlers.rs` and
`process_manager.rs` are **byte-identical** between `v0.4.0-rc-5` and
`3e378636`. Upstream RC5 did improve this area (it replaced v0.4.0's
`pwsh "cmd /C \"{}\""` wrapper with extension-based dispatch, and added a manual
handler selector), but the POSIX tokenisation underneath it is untouched.

**Approach.**
1. A Windows tokeniser/quoter (`windows_words`): double quotes group, `""` is a
   literal quote, backslash is an ordinary character. Used by
   `parse`/`reconstruct` **on Windows only** — Unix keeps upstream's POSIX
   behaviour verbatim.
2. `coalesce_unquoted_command(base)`: syntax alone cannot distinguish
   "one file with a space" from "a command plus an argument", so it asks the
   filesystem — greedily merging leading tokens only while the candidate
   actually exists. `Game.exe --windowed` keeps its argument (`Game.exe`
   resolves first); `notepad` is left alone for PATH resolution.
3. Called from `windows_launch_command` **before** extension sniffing.

Supported forms: `Game.exe`, `"Game With Spaces.exe"`, `Game.exe --arg`,
`"Game With Spaces.exe" --arg`, and bare `Game With Spaces.exe`.

**Dependencies.** `tempfile` as a **dev**-dependency only. No runtime
dependency added.

**Tests.** 15 unit tests in `parser.rs` — tokenising, quoting, round-tripping,
and every coalescing branch. Run with:

```bash
cd desktop/src-tauri && cargo test -p process --lib
```

The `windows_words` module is compiled under `cfg(any(target_os = "windows", test))`
so the rules stay covered even when CI runs on Linux.

Manual TEST 4–5 in the test plan.

**ZougCloud commit.** `0d513567`

**Upstream reference.** None. Related upstream work: `9185089c` ("Fix v0.4.0
process handler, add override menu", #430) and `6f747186` ("Fix local path and
templating issues", #436) — both already in our base and both stopping short of
the tokeniser.

**State.** `custom` — this is a strong upstream PR candidate. If offered and
merged, drop the patch.

---

## Infrastructure — client-only guard

**Problem solved.** Nothing technical prevented a future change from editing
`server/` and silently turning this into a client+server fork.

**Files.** `scripts/verify-client-only.ps1`,
`.github/workflows/zougcloud-client-only.yml`

**Notes.** Default-deny allow-list; three-dot diff against the merge base so
merged upstream work is never attributed to us. Verified in both directions
(passes on our patches, exits 1 on a simulated `server/` edit).

**ZougCloud commit.** `469c7dbb`

**State.** `custom` — permanent infrastructure, never upstreamable.

---

## ZC-004 — Steam shortcut integration

**Problem solved.** Drop is a good installer and a plain launcher. Steam gives
members the overlay, controller support, playtime tracking and a library they
already live in. Adding a Drop game to Steam by hand means finding the
executable, getting the working directory right, and redoing it after a move.

**Files.**
- `desktop/src-tauri/steam/` (new crate: `error.rs`, `locate.rs`, `shortcuts.rs`)
- `desktop/src-tauri/process/src/resolve.rs` (new)
- `desktop/src-tauri/src/steam.rs` (new)
- `desktop/src-tauri/Cargo.toml`, `process/src/lib.rs`, `src/lib.rs` (registration only)
- `desktop/main/components/GameOptions/Steam.vue` (new), `GameOptionsModal.vue`

**Why upstream is not enough.** Upstream has no Steam integration at all.

**Client-only.** Confirmed: everything comes from the local database
(install dir, launch command, args) and from Steam's own files under
`userdata/<account>/config/`. No Drop server call, no new endpoint.

**Design notes.**
- The shortcut points at the **game** executable, never `drop-app.exe`, and
  never at a `cmd.exe`/PowerShell wrapper — otherwise Steam records playtime for
  the launcher or the shell instead of the game. This is why
  `process::resolve` stops before `create_launch_process`.
- Launch resolution reuses `ParsedCommand`, so ZC-003's handling of executables
  with spaces applies identically here.
- Emulator launches are skipped: there is no single executable to point at.
- **Steam is closed and restarted around a write**, not refused. Steam rewrites
  `shortcuts.vdf` from memory on exit and only scans the artwork folder at
  startup, so editing either while it runs is pointless. Refusing left the
  member to work out what to do; closing and restarting also makes the result
  visible immediately. It is `steam.exe -shutdown` — Steam's own graceful exit,
  never a kill — and it declines while a game is running, which is surfaced
  ("quit the game, then try again") rather than forced. Steam is restarted even
  on the failure path.
- `shortcuts.vdf` holds **only** non-Steam shortcuts. A genuine licence lives in
  `steamapps/appmanifest_*.acf`, which this code never opens, so a real Steam
  copy of the same game cannot be touched.
- Rolling backups (5) plus an atomic rename, because a truncated write would
  cost the member every non-Steam shortcut they have, Drop's or not.

**Dependencies.** `steamlocate` 2.1.1 (maintained), `steam_shortcuts_util`
1.1.8, `sysinfo`, `keyvalues-parser`. `steam_shortcuts_util` was last published
in 2022; it was adopted after a spike confirmed it compiles on our nightly /
edition 2024 and round-trips correctly, and it is backed by our own tests rather
than trusted blindly. The binary VDF shortcut format has been stable for years.

**Tests.** 12 unit tests in the `steam` crate covering app-id stability,
Steam's quoting convention, round-trip, deduplication, preservation of unrelated
shortcuts, backups, and removal. Manual TEST 7, 8, 10 in the test plan.

Also verified against the real Steam install on the build machine: two accounts
found with persona names, existing hand-made shortcut correctly reported as not
Drop-managed.

**ZougCloud commits.** `415daa63` (crate), `c412152f` (launch resolution),
`21a753a4` (commands + UI)

**State.** `custom` — upstream has nothing comparable.

---

## ZC-006 — Steam shortcut management

Delivered as part of ZC-004 rather than as a separate patch, because the
guarantees are properties of the same write path.

- **Already added?** `steam_game_status` reports the existing shortcut.
- **Open in Steam.** `steam://nav/games/details/<id>`; safe while Steam runs.
- **Remove.** Explicit user action only. A Drop game update never removes a
  shortcut.
- **A moved game keeps its identity.** `upsert_shortcut` reuses the *existing*
  app id instead of the freshly generated one. Steam keys playtime on that id
  and names artwork files after it, so reusing it preserves both. Matching order
  is app id → Drop-tagged shortcut with the same name → executable path; the
  middle rule is what catches a changed install directory.

Covered by `a_moved_game_keeps_its_app_id_and_playtime`. Manual TEST 11.

**State.** `custom`

---

## ZC-005 — Steam artwork

**Problem solved.** A non-Steam shortcut with no artwork is a grey box with a
filename. Members would have to find and crop five images per game by hand.

**Files.**
- `desktop/src-tauri/steam/src/artwork.rs` (new)
- `desktop/src-tauri/src/steamgriddb.rs` (new)
- `desktop/src-tauri/src/steam.rs`, `desktop/main/components/GameOptions/Steam.vue`

**Why upstream is not enough.** Upstream has no Steam integration at all.

**Client-only.** SteamGridDB is a third party; the fallback uses
`api/v1/client/object/{id}`, an endpoint that already exists and that the
library itself uses. No server change.

**Design notes.**
- Five slots, named after the shortcut's app id in
  `userdata/<account>/config/grid/`. There is no index — the filename *is* the
  association, which is why ZC-006 preserves the app id across a move.
- The capsule stem (`123`) is a prefix of the portrait's (`123p`), so lookups
  compare whole stems. Prefix matching would delete one while writing the other.
- The extension comes from the image's magic bytes, and any existing file for
  the slot is removed first whatever its extension: Steam dispatches on the
  extension, so a JPEG written as `.png` renders blank and a stale `123p.jpg`
  would beat a fresh `123p.png`.
- Applied inside the same Steam-closed window as the shortcut, so one click
  produces a complete entry.
- Every artwork failure is swallowed and logged. Artwork is a nicety; losing it
  must never fail the thing the member asked for.
- Logo has no Drop equivalent (it is a transparent title treatment), so that
  slot is left empty rather than filled with a cover that would sit wrongly over
  the hero.

**The API key.** No key ships with the client. A hardcoded one would be
committed to a public AGPL repository, shared by every member, and revoked as
soon as anyone noticed. The user's key lives at
`%APPDATA%\drop\zougcloud\steamgriddb.key` — deliberately not in `drop.db`, to
keep it out of the database blob and its backups, and to avoid adding a field to
an upstream model that would need reconciling at every rebase. It is write-only
across the IPC boundary: the frontend can set or clear it and learn *whether*
one exists, never read it back.

**Tests.** 7 unit tests in `artwork.rs` (stems, extension detection, the
capsule/portrait collision, extension replacement, removal isolation).
Manual TEST 9.

**ZougCloud commits.** `3ea8b514` (crate), `fed5ac6e` (fetching + UI)

**State.** `custom`
