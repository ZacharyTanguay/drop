# The ZougCloud fork of Drop Desktop

> **If you read one thing:** this fork changes the **Windows Desktop client only**.
> The Drop server stays byte-for-byte upstream. That rule is absolute, and
> `scripts/verify-client-only.ps1` enforces it.

---

## Why this fork exists

ZougCloud runs a stock Drop server. The Windows Desktop client, however, has
three defects that make it frustrating for our members:

1. Closing the window leaves the process alive in the tray, and relaunching the
   shortcut does nothing — users end up killing Drop from the Task Manager.
2. A newly published game version is not noticed until the client is restarted.
3. A game whose executable name contains a space (`Graveyard Keeper.exe`) fails
   to launch.

All three are fixable **inside the client**. None of them needs a server change.
That is the entire premise of this fork: get a better client without giving up
the ability to pull future Drop OSS releases.

On top of the fixes, one addition: optionally adding an installed game to Steam
as a non-Steam shortcut, with artwork. Drop stays the installer, updater and
version manager; Steam becomes the launcher for members who want the overlay,
controller support and playtime tracking. Also entirely client-side — it reads
and writes Steam's own files and, optionally, SteamGridDB.

### What we deliberately do *not* do

- No server fork, no new API, no DB migration, no custom Docker image.
- No dependency on a `develop` or custom server.
- No cloud-save system (explicitly out of scope; possibly a separate project).

---

## The client-only rule

**We may change:**

| Path | Why |
|---|---|
| `desktop/` | The client. Our entire remit. |
| `docs/ZOUGCLOUD-*.md`, `docs/UPSTREAM-UPDATE-PROCEDURE.md` | These documents. |
| `scripts/` | Our build and guard scripts. |
| `.github/workflows/zougcloud-*.yml` | Our own CI, namespaced. |

**We may not change anything else.** The policy is *default-deny*: a new
upstream top-level directory is refused on sight rather than quietly slipping
through.

A few paths deserve special mention because they are less obvious than
`server/`:

- **`libraries/base/`** — a Nuxt layer **shared** by `desktop/main` *and*
  `server/`. Editing it changes the server build even though it lives outside
  `server/`. Forbidden.
- **`pnpm-lock.yaml`, `pnpm-workspace.yaml`, `package.json`** (root) — pin
  server dependencies. Forbidden. (`desktop/main/pnpm-lock.yaml` and
  `desktop/src-tauri/Cargo.lock` are inside `desktop/` and therefore fine.)
- **`torrential/`, `backend/`, `cli/`, `Dockerfile`** — server-side. Forbidden.

Run the check any time:

```bash
powershell -ExecutionPolicy Bypass -File scripts/verify-client-only.ps1 -IncludeWorkingTree
```

CI runs it automatically on every `zougcloud/**` push and PR
(`.github/workflows/zougcloud-client-only.yml`).

### If you ever *need* a server change

Stop. Do not add a workaround to the backend. Either solve it inside
`desktop/`, or take it upstream as a PR to `Drop-OSS/drop`. A server change
means this fork has failed its purpose, and that is a decision for the fork
owner, not for an implementer.

---

## Current baseline

| | Value |
|---|---|
| Upstream repo | `https://github.com/Drop-OSS/drop.git` (remote `upstream`) |
| Fork remote | `origin` |
| Our branch | `zougcloud/client` |
| **Upstream base commit** | `3e378636579c67aae5f7a0026b03ea6f88243f9e` (`upstream/develop`, 2026-08-21) |
| **Server we must stay compatible with** | `ghcr.io/drop-oss/drop:0.4.0-rc-5` = commit `6f7471869515e3a61121ae2af2d556d8914d30e4` |

### Why base on `develop` rather than the `v0.4.0-rc-5` tag

Because it is provably safe *and* strictly less work to maintain.

Between `v0.4.0-rc-5` and `3e378636` the Desktop changes in exactly two files —
`desktop/main/app.vue` (+4) and `desktop/src-tauri/src/lib.rs` (+28/−6), both
from upstream commit `12c81eb1`. Everything else in `desktop/` is a dependency
bump. Critically, these directories have a **strictly empty diff**:

```
desktop/src-tauri/remote/            <- the API layer
desktop/src-tauri/games/
desktop/src-tauri/download_manager/
desktop/src-tauri/process/
desktop/src-tauri/database/src/
desktop/src-tauri/client/src/
```

The `develop` client therefore issues **byte-identical** requests to the
`v0.4.0-rc-5` client. Compatibility with our server is guaranteed by
construction, not by testing. And basing here gives us the upstream half of
ZC-001 for free.

### The API surface we depend on

Twelve routes, all present in the RC5 server:

```
/api/v1                                  (health probe)
/api/v1/client/auth/initiate
/api/v1/client/auth/handshake
/api/v1/client/auth/code/ws              (WebSocket)
/api/v1/client/user
/api/v1/client/user/webtoken
/api/v1/client/user/library
/api/v1/client/game
/api/v1/client/game/manifest
/api/v1/client/collection
/api/v1/client/depots
```

**There is no version negotiation.** The probe only checks that the server
reports `app_name == "Drop"`. At `auth/initiate` the client sends `name`,
`platform`, `capabilities` (`peerAPI`, `cloudSaves`) and `mode` — never a
version string. Changing the Desktop version number is therefore safe and
affects nothing server-side.

> **Watch item for a future server upgrade.** Upstream `develop` already
> modifies two client routes (`client/user/library.get.ts`,
> `client/collection/default/entry.post.ts`) for its age-restriction and groups
> features. They are irrelevant while our server stays on RC5, but must be
> re-read before the server is ever moved forward.

---

## There is no auto-updater

Verified at both `v0.4.0-rc-5` and `3e378636`:

- no `tauri-plugin-updater` in `desktop/src-tauri/Cargo.toml`;
- no `updater` key in `desktop/src-tauri/tauri.conf.json`;
- no self-update code anywhere in `desktop/`.

The Tauri bundler even says so during a build: *"Updater plugin may not be able
to update this package."*

**Nothing can silently replace a ZougCloud build with an official one.** We
distribute installers by hand. Nothing needs to be disabled.

> If upstream ever adds an updater, the upstream-update procedure will catch it
> — and we must then either point it at our own releases or remove the plugin,
> **before** shipping that build.

---

## Licence and redistribution

`desktop/LICENSE` is the **GNU Affero General Public License v3**
(© 2024 DecDuck). Because we hand a modified binary to our members, we are
"conveying" it, and AGPL §6 applies:

- **We must offer the corresponding source** of the exact build we distribute.
  Keeping this fork public on GitHub satisfies that cleanly — ship the commit
  hash in `BUILD-INFO.txt` alongside every installer so the source of *that*
  build is identifiable.
- **Our modifications stay AGPL-3.0.** We do not relicense anything.
- **Keep upstream notices intact** and mark modified files (our
  `ZOUGCLOUD(ZC-xxx)` comments serve as the modification markers).

§13 (source for remote network users) does not trigger here: the Desktop is a
client, not a network-facing service.

**Never change an upstream `LICENSE` file.** The AGPL forbids it, and
`verify-client-only.ps1` blocks it anyway.

---

## Working on the fork

### Branch and commit conventions

All work lands on `zougcloud/client` as **small, independent commits**, one per
patch:

```
zougcloud: ZC-001 restore the window when a second instance is launched
zougcloud: ZC-002 emit update_game so the UI notices new versions
zougcloud: ZC-003 parse Windows launch commands with Windows rules
```

This is not cosmetic. It is what lets a future maintainer drop a single patch
once upstream fixes that specific bug, without untangling it from four others.

### Marking our code

Every non-obvious divergence carries a searchable marker explaining **why** the
patch exists, not what the code does:

```rust
// ZOUGCLOUD(ZC-003): Windows command lines are not POSIX shell words. [...]
```

To see every divergence at a glance:

```bash
git grep -n "ZOUGCLOUD("
```

Keep `docs/ZOUGCLOUD-PATCHES.md` in step with what that command returns.

---

## Building

Full prerequisites and exact toolchain versions live in the generated
`BUILD-INFO.txt`. In short:

```bash
pnpm install
```
```bash
cd desktop && pnpm tauri build
```

The installer lands at:

```
desktop/src-tauri/target/release/bundle/nsis/
```

Only NSIS is produced on Windows — `tauri.conf.json` sets `"wix": null`, so
there is no MSI. That is upstream's configuration and we do not change it.

For a guaranteed-clean rebuild, delete `desktop/src-tauri/target`,
`desktop/.output` and `desktop/main/.output` first.

### Before every build

```bash
cd desktop/src-tauri && cargo test -p process --lib
```
```bash
powershell -ExecutionPolicy Bypass -File scripts/verify-client-only.ps1
```

> `cargo check --workspace` fails on `cloud_saves`. That breakage is
> **pre-existing upstream** and unrelated to our patches — `cloud_saves` is a
> workspace member but not a dependency of `drop-app`, so `tauri build` never
> compiles it. Use plain `cargo check` (which builds `drop-app` and its real
> dependencies) as the gate.

---

## Testing

- Automated: `cargo test -p process --lib`.
- Manual: **`docs/ZOUGCLOUD-TEST-PLAN.md`** — run it before distributing any
  installer.

---

## Publishing an installer

1. Bump the version in **both** `desktop/src-tauri/tauri.conf.json` and
   `desktop/src-tauri/Cargo.toml`, then run `cargo check` in
   `desktop/src-tauri` **before committing**. Cargo rewrites `Cargo.lock` with
   the new `drop-app` version; committing without that step leaves the lockfile
   one commit behind, and `package-release.ps1` will refuse the build.
2. Confirm the working tree is clean and tests pass.
3. Build (above).
4. Copy the NSIS `.exe` out of `target/` into a keep-safe directory.
5. Generate `SHA256SUMS.txt` and `BUILD-INFO.txt` next to it, recording the
   upstream base commit, the ZougCloud commit, date, toolchain versions,
   architecture, SHA-256 and the exact build command.
6. Tag the commit you shipped (e.g. `zougcloud/0.4.0-zc.1`) so the AGPL source
   offer points at something precise.
7. Hand the `.exe` to members.

The installer upgrades a previous ZougCloud install in place. It preserves
`%APPDATA%\drop` — games, database, config — because the Tauri NSIS uninstaller
only ever removes `%APPDATA%\org.droposs.client` and
`%LOCALAPPDATA%\org.droposs.client`, and only when *both* the "delete
application data" box is ticked *and* it is not an upgrade
(`installer.nsi`: `$DeleteAppDataCheckboxState = 1` **and**
`$UpdateMode <> 1`). Drop's real data directory is never a target.

---

## Rollback

**A bad client build.** Reinstall the previous ZougCloud installer; it upgrades
in place and leaves `%APPDATA%\drop` alone. Keep the last known-good `.exe` and
its `BUILD-INFO.txt`.

**A bad patch, not yet shipped.** Because each patch is its own commit:

```bash
git revert <commit>
```

**Back to stock upstream Desktop.** Install an official Drop release. The local
database is compatible in both directions: it is RON (self-describing, named
fields), the version enum is unchanged, and no struct uses
`serde(deny_unknown_fields)` — so an older client ignores fields it does not
know.

> Back up `%APPDATA%\drop\drop.db` before any downgrade experiment. Cheap
> insurance: if the client cannot parse it, `handle_invalid_database` renames it
> to `drop.db.backup-<timestamp>` and starts empty, which loses the *record* of
> installed games (the game files themselves stay on disk).

---

## Keeping up with upstream

See **`docs/UPSTREAM-UPDATE-PROCEDURE.md`**. That document is written to be
handed to a future session verbatim: *"Drop OSS just released a new version,
update our fork."*
