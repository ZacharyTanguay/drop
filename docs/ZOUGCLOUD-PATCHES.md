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
| ZC-004 | Steam shortcut integration | *planned* | — |
| ZC-005 | Steam artwork | *planned* | — |
| ZC-006 | Steam shortcut management | *planned* | — |
| — | Client-only guard (infrastructure) | `custom` | `469c7dbb` |

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

## Planned

### ZC-004 — Steam shortcut integration
Add an installed Drop game to Steam as a non-Steam shortcut, pointing at the
**game** executable (never `drop-app.exe`, or playtime would be attributed to
the launcher). Feasibility confirmed client-side: install dir, launch command,
args and working directory are all already in the local database.

### ZC-005 — Steam artwork
Grid/capsule, hero, logo and icon for the shortcut. SteamGridDB as an optional
source behind a user-supplied API key (never committed, never hardcoded), with a
fallback to Drop's own images — the `Game` model already carries
`m_icon_object_id`, `m_banner_object_id`, `m_cover_object_id` and the library /
carousel image ID lists.

### ZC-006 — Steam shortcut management
Detect whether a game is already added, "Open in Steam", explicit
"Remove from Steam". A Drop game update must **never** silently remove the
shortcut or its artwork; only a changed install path updates the entry.

**Shared risk for ZC-004/005/006.** `shortcuts.vdf` is binary VDF and Steam
rewrites it on exit. Candidate crates: `steamlocate` 2.1.1 (actively
maintained, 2026-08) for discovery; `steam_shortcuts_util` 1.1.8 for the binary
format — widely used but last published 2022-05, so it needs an explicit
evaluation before adoption. Back up before writing, and never write while Steam
may overwrite.
