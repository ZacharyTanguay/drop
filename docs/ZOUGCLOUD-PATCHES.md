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
| ZC-007 | GOG Galaxy integration | **`abandoned`** | — |
| ZC-008 | Local playtime tracking | `custom` | `9a63e2f3`, `3bcac577` |
| ZC-009 | Open in Steam on game pages | `custom` | `700efb32` |
| ZC-010 | Managed Tailscale | *in progress* | — |
| ZC-011 | Client-side game access | `custom` | `a283ae7c`, `7631a69c`, `3faf68bb`, `384bd3ed`, `357e1cd8` |
| ZC-012 | Global error recovery | `custom` | `ff625cbf`, `104eed62` |
| ZC-013 | Access modes, pricing, interests | *partial* | `a283ae7c`, `384bd3ed` |
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

---

## ZC-007 — GOG Galaxy integration — **ABANDONED / BLOCKED**

**Reason.** GOG Galaxy requires every integration to declare a **Platform ID
from a fixed official list**, and there is no Drop or ZougCloud entry on it —
nor any generic, custom or community option among the 99 IDs.

New IDs are granted only by GOG. Their process
([issue #160](https://github.com/gogcom/galaxy-integrations-python-api/issues/160),
still open) states it *"requires business acceptance, handlers in other
repositories and preparation of platform metadata"*; dozens of community
requests have sat there unresolved for years.

The only available workaround is `test`, documented as **"Testing purposes"**.
Galaxy groups the library by platform and takes those labels from its own
metadata — the local Galaxy database on the build machine holds "Epic Games
Store" and "Humble Bundle" but nothing for `test`. Members' ZougCloud games
would therefore land in an unnamed test bucket rather than under a ZougCloud
identity, shared with any other community plugin that also picked `test`. The
manifest `name` shows in the integrations list, not as the library platform.

Patching Galaxy's own database to fake a platform is excluded: fragile, and it
would break on any Galaxy update.

**Do not reopen this without a granted Platform ID.** If GOG ever assigns one,
the work becomes small: the local contract built for ZC-008
(`%APPDATA%\drop\zougcloud\`) is already the versioned, Drop-independent source
a plugin would read.

**State.** `abandoned`

---

## ZC-008 — Local playtime tracking

**Problem solved.** Drop had no idea how long anyone had played. Steam only
counts what it launched, and the Drop server holds nothing.

**Files.**
- `desktop/src-tauri/playtime/` (new crate: `model.rs`, `store.rs`, `tracker.rs`, `format.rs`)
- `desktop/src-tauri/src/playtime.rs` (new: command + watcher)
- `desktop/main/composables/zougcloud.ts`, `desktop/main/components/ZougcloudPlaytime.vue` (new)
- `desktop/src-tauri/process/src/process_manager.rs` (2 hooks), `src/lib.rs`,
  `desktop/main/pages/library/[id]/index.vue` (registration and display only)

**Why upstream is not enough.** Upstream has no playtime concept at all.

**Client-only.** Nothing leaves the machine. Stored at
`%APPDATA%\drop\zougcloud\playtime.json`, deliberately **outside `drop.db`** so
an upstream schema change can never cost a member their hours and so a rebase
does not touch it.

**The invariant: one active session per game, with an owner.**
`begin_session` is a no-op when a session is already open; `end_session` only
closes one belonging to the same owner. Drop's process manager (`Drop`) and the
external watcher (`Watcher`) therefore cannot double count, and this is a
property of the tracker rather than of careful call ordering.

- **Launched from Drop** — the session opens *after a successful spawn*, not
  when Play is clicked, so a failed launch credits nothing; it closes when the
  process exits.
- **Launched from Steam** — a watcher polls every **7 s** (coarse on purpose: it
  runs for the life of the app, and seconds of imprecision do not matter here).
  The watch list is rebuilt only every 60 s, since resolving launch targets
  touches the database and disk.

**Crash recovery.** A session is credited up to its **last heartbeat**, never up
to the current time. A machine can sit powered off overnight between a crash and
the next launch; crediting that gap would invent hours nobody played. A session
that crashed before its first heartbeat credits nothing.

**Storage safety.** Writes go through a temp file, an `fsync` and an atomic
rename. A file that cannot be parsed is moved to
`playtime.corrupt-<timestamp>.json` and a fresh one started — never silently
replaced, because the original may still be salvageable by hand.

**Known limitation.** External tracking covers **direct `.exe` targets only**. A
launch command that goes through a `.bat`, `cmd`, PowerShell or an emulator
spawns a process we did not name and cannot attribute to a game, so those are
skipped rather than guessed at. Games launched *from Drop* are tracked
regardless of launch type, because the process manager knows the lifecycle.

**Tests.** 23 unit tests: session arithmetic, no-double-count, orphan recovery
stopping at the heartbeat, backwards clocks, atomic writes, corrupt-file
preservation, persistence, and every formatting branch. Manual TEST 12–15.

**ZougCloud commits.** `9a63e2f3` (crate), `3bcac577` (hooks + watcher)

**State.** `custom`

---

## ZC-009 — Open in Steam on game pages

**Problem solved.** The Steam integration existed but was reachable only from
the game options modal. The game page now shows playtime beside the update
status, and an **Open in Steam** button beside Play.

**Files.** `desktop/main/composables/zougcloud.ts`,
`desktop/main/components/ZougcloudPlaytime.vue`,
`desktop/main/pages/library/[id]/index.vue`,
`desktop/src-tauri/steam/src/shortcuts.rs`, `desktop/src-tauri/src/steam.rs`

**Design notes.**
- Visibility comes from reading Steam's own shortcuts file on each refresh, not
  from a cached flag: deleting the shortcut from inside Steam must make the
  button disappear, and a local boolean would keep claiming it exists.
- Fixed the URL: `steam_open_shortcut` was passing the 64-bit `run_game_id` to
  `nav/games/details`. That form belongs to `rungameid`, which **launches** the
  game — the opposite of what this button is for. The library keys non-Steam
  entries on the 32-bit shortcut id.
- **Read-only by construction.** There is no write path, which is what preserves
  the AppID, artwork, Steam playtime and controller settings.
- The button row gained `flex-wrap` so a fourth button wraps on narrow windows.

**Limitation.** Steam's handling of `steam://nav/games/details/<id>` has been
unreliable across client versions. The failure mode is benign — Steam comes to
the foreground on the library rather than doing nothing — so there is no
fallback to detect or code. Faking selection by editing a Steam database is
excluded.

**Tests.** 2 unit tests: the URL navigates and never launches, and opening
leaves the shortcuts file, artwork and AppID untouched. Manual TEST 16–17.

**ZougCloud commit.** `700efb32`

**State.** `custom`

---

## ZC-010 — Steam shortcut icon — **not created, and should not be**

Steam showed a generic monitor icon in its left-hand list for Graveyard Keeper.
The cause was not our shortcut writer: the launch command still pointed at
`Launch.bat`, a historical workaround from before ZC-003 fixed executables with
spaces. A `.bat` has no embedded icon, so Steam had nothing to show.

Pointing the launch command at the real `Graveyard Keeper.exe` makes Steam pick
up the game's own icon natively.

**No ZougCloud patch is required.** Do not add icon extraction, local `.ico`
generation or an icon repair path — they would all be solving a configuration
mistake in code.

---

## ZC-011 — Client-side game access

> **This is UX steering, not authorisation.** The decision runs entirely in the
> client. Anyone technical can bypass it by using the stock Drop client or
> calling the server API directly. It exists to shape what non-technical
> members see. Drop Server is deliberately unchanged.

**Files.** `desktop/src-tauri/access/` (new crate),
`desktop/src-tauri/src/access_provider.rs` (new),
`desktop/src-tauri/client/src/user.rs` (two getters), `src/lib.rs` (wiring).

### The model

```
game.accessMode  : free | gated
user.accessMode  : custom | all
```

An earlier draft put `all` on the *game*. That was wrong: "everyone gets
everything" is a property of a **member**, and the two answer different
questions. Corrected at schemaVersion 2 while the live manifest was still
empty — the last moment it was free to do.

Precedence, and the ordering matters:

1. the admin bypasses everything, **before** the manifest is consulted, so a
   missing or unfetched manifest can never lock them out of their own client;
2. a member whose mode is `all` gets every game, **before** the game is looked
   up — that is what makes "All games" cover games not yet in the manifest, so
   the admin need not revisit every all-member on each import;
3. `free` needs no grant;
4. `gated` needs an explicit grant;
5. anything else fails closed.

Both defaults lean restrictive: a member absent from the manifest is `custom`,
and an `accessMode` this build cannot read falls back to `custom`. Reading
either the other way would hand the catalogue to anyone who signs in.

### Identifiers

Games and members are keyed on **Drop UUIDs** — `User.id` is
`@default(uuid())` in the server schema. Never on username or title: those are
guessable and mutable.

### Prices

Integer minor units (`1299` = $12.99). Never floats. `price: null` means "no
price configured" and is **not** the same as free — a gated game can be
priceless and still need a grant.

### The last-known-good contract

| Situation | Result |
|---|---|
| Never fetched successfully | Deny — `free`/`all` cannot be assumed without an authoritative copy |
| Fetched before, remote now unavailable | **Keep applying the cached manifest** |
| 304 Not Modified | Keep the cached manifest |
| New response malformed | Reject it, keep the cache |
| New response from a newer schemaVersion | Reject it, keep the cache |
| Newer valid revision | Atomic replacement |

A GitHub outage must never erase a valid policy: a member whose library worked
yesterday must not lose it because a remote had a bad day. Equally, no failure
path may widen access.

### The provider

Members read `raw.githubusercontent.com/ZacharyTanguay/zougcloud-games-access`
with **no credential** — which is why the repository is public and holds only
opaque UUIDs. Conditional requests with ETag / If-None-Match.

Polling every 5 minutes matches the `Cache-Control: max-age=300` the raw
endpoint is currently observed to send; polling faster would only re-read a
cached copy. That is an observation, not a protocol assumption — correctness
rests on the conditional request.

**Propagation latency, measured.** An admin write lands on the GitHub API
immediately, but the raw CDN serves the old copy for up to 5 minutes, and a
member then waits up to one poll interval. Worst case is therefore about
**10 minutes** for an access change to reach a member. Verified empirically:
after a write the API returned revision 2 while raw still served revision 1.

### Admin write

GitHub Contents API with a fine-grained PAT that lives only in the admin's
Windows Credential Manager under `ZougCloud/GitHubToken`. `keyring`'s
`new_with_target` is used because on Windows the target name is a credential's
sole identifier, so this reads exactly what `cmdkey` stored.

The token is passed straight into a request header. It is never logged, never
placed in an error message, and never written to disk. It is not in the
repository, the installer, or BUILD-INFO.

**Tests.** 44 unit tests in the `access` crate plus 4 in the provider. An
`#[ignore]`d end-to-end test hits the real repository; it is non-destructive by
construction, incrementing `revision` and carrying every policy and grant
through untouched.

**State.** `custom`

---

## ZC-012 — Global error recovery

**Problem solved.** Drop could land on a black screen showing
`asset not found: main/id/me`, escapable only with mouse-Back then F5.

Two independent defects, both upstream:

1. `HeaderUserWidget.vue` linked to `/id/me` and the Desktop has **no
   `pages/id/` route at all**. In a Tauri SPA an unknown route becomes an asset
   request. Pointed at `/settings/account`, which exists; no route was invented
   to paper over it.
2. `error.vue`'s only way out was `<a href="/store">` — a raw href that ignores
   the `/main` baseURL and produces another asset error. It also rendered inside
   `NuxtLayout default`, which mounts the header, which awaits the user object
   and a Tauri command — so an error caused by bad app state risked failing the
   error page too.

The new page has no layout and no data dependencies, and recovers through
`clearError({ redirect })`, which drops Nuxt's error state *before* routing.
Retry is withheld for a missing route: retrying one would fail identically and
walk the user into the loop the page exists to prevent.

Classification is a pure, tested function preferring structured signals (status
code, connection hint) over message text; text is consulted only for the Tauri
asset case, which carries no status code.

**Tests.** 8 frontend tests (vitest, added — the Desktop had no frontend test
infrastructure).

**State.** `custom` — both defects exist upstream and are PR candidates.

---

## Tailscale external identity — validated

**External Machine Sharing + Tailscale Serve identity: ✅ EMPIRICALLY
VALIDATED.**

Test account: `zacktanguay2@gmail.com` — a distinct Tailscale account, on its
own tailnet, which accepted the share of `zougcloud-games` and opened a Serve
endpoint.

Result: Serve returned **that account's own** `Tailscale-User-Login` and
`Tailscale-User-Name`, not the sharer's.

A negative control was taken first: reaching the same backend directly over
loopback returned `(absent)` for both headers. So the headers demonstrably come
from Serve, and a client cannot forge them by bypassing it — which is exactly
why the backend must listen on localhost only.

Two constraints discovered during the test, which the interest service design
must respect:

- the service must be reachable on **443**; the tailnet ACL grants
  `autogroup:shared` only `tcp:443` on `zougcloud-games`, and an earlier attempt
  on 8443 failed for that reason;
- the admin identity for the service is `zacktanguay@gmail.com`, which is
  distinct from the Drop username `zacktanguay` that drives Desktop admin UX.

The Drop-JWT alternative is not needed and must not be reintroduced.
