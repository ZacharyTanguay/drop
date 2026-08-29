# Upstream update procedure

> **Written to be handed over verbatim.** If you were told *"Drop OSS just
> released a new version, update our fork"*, this is the runbook. Read
> `docs/ZOUGCLOUD-FORK.md` first for the rules you must not break.

**The one rule.** ZougCloud changes the **Desktop client only**. The server
stays stock upstream. If an upstream change makes a ZougCloud goal impossible
without a server change, **stop and report** — do not work around it in the
backend.

---

## Step 0 — Establish where we are

```bash
git fetch upstream --tags --prune
```

Record, and put these in the final report:

```bash
git rev-parse HEAD                                   # our branch tip
git merge-base HEAD upstream/develop                 # our current upstream base
git log --oneline $(git merge-base HEAD upstream/develop)..upstream/develop
```

Also note the server we must stay compatible with. **It is authoritative, not
negotiable** — check the actual deployed tag, do not assume:

```bash
docker inspect --format='{{index .Config.Image}}' <drop-container>
```

Current baseline is recorded in `docs/ZOUGCLOUD-FORK.md` → *Current baseline*.

---

## Step 1 — Identify the new release

```bash
git tag --list --sort=-creatordate | head
git log -1 --format='%H %ci %s' <new-tag>
```

Confirm our current base is an ancestor (i.e. this really is a fast-forward of
history, not a divergent branch):

```bash
git merge-base --is-ancestor HEAD upstream/develop && echo "ancestor OK"
```

---

## Step 2 — Split the diff by area

This single command tells you most of what you need:

```bash
git diff --stat <our-base> upstream/develop -- desktop/
```
```bash
git diff --stat <our-base> upstream/develop -- server/ backend/ libraries/
```

Ignore lockfiles and dependency bumps when judging risk; look for real code.

---

## Step 3 — Identify Desktop changes

```bash
git diff --stat <our-base> upstream/develop -- 'desktop/**/*.rs' 'desktop/**/*.vue' 'desktop/**/*.ts'
```

Read every hunk that touches a file any ZC-xxx patch also touches
(`docs/ZOUGCLOUD-PATCHES.md` lists them per patch).

---

## Step 4 — Identify server API changes

The routes our client actually calls:

```bash
git diff <our-base> upstream/develop -- server/server/api/v1/client/
```

Cross-check against the client's real API surface — regenerate it rather than
trusting this list:

```bash
git grep -n -o -E '"/api/v1/[a-zA-Z0-9/_.-]*"' upstream/develop -- desktop/src-tauri
```

Also diff the API layer itself. **If this is empty, the new client speaks
exactly the same protocol as the old one and compatibility is guaranteed by
construction** — this is the single most valuable check in the whole procedure:

```bash
git diff --stat <our-base> upstream/develop -- desktop/src-tauri/remote/ desktop/src-tauri/games/ desktop/src-tauri/download_manager/ desktop/src-tauri/client/src/
```

---

## Step 5 — Verify compatibility with **our** server

The question is never "is the new client newer" — it is:

> **Does the new Desktop call anything our RC5 server does not implement, or
> assume a response shape RC5 does not produce?**

Work through:

1. **New endpoint called by the client?** Check it exists at our server commit:
   `git ls-tree -r --name-only <server-commit> server/server/api/v1/client/`
2. **Changed request DTO** on an endpoint we call?
3. **Changed response DTO** the client now depends on (new required field)?
4. **Auth / OIDC / handshake / webtoken** changes?
5. **WebSocket** (`/api/v1/client/auth/code/ws`) protocol changes?
6. **Manifest / depot / Torrential** format changes — these are the
   download path and the most dangerous to get wrong.
7. **Prisma migrations** that alter a contract the client consumes:
   `git diff --stat <our-base> upstream/develop -- server/prisma/`
8. **Version negotiation** — as of the RC5 baseline **none exists**: the probe
   only checks `app_name == "Drop"` and the client never sends a version.
   Re-verify this is still true rather than assuming it.

**If the new Desktop requires a newer server: STOP.** Report which feature, why
it cannot be done client-side, and what server change it would need. Do not
upgrade the server to follow the client.

**Fallback if only part of it is incompatible:** stay on the current base and
cherry-pick only the compatible client commits.

---

## Step 6 — Check for an auto-updater

Historically there is none, and that is load-bearing: it is why our custom build
cannot be silently replaced by an official one.

```bash
git grep -n -i "updater" upstream/develop -- desktop/src-tauri/Cargo.toml desktop/src-tauri/tauri.conf.json
```

**If upstream has added one, it must be handled before shipping** — either point
it at our fork's releases or remove the plugin. Never ship a build that can
overwrite itself with upstream's.

---

## Step 7 — Rebase our patches

```bash
git checkout -b zougcloud/client-<newversion> zougcloud/client
git rebase --onto upstream/develop <our-base>
```

Resolve conflicts **one patch at a time**. That is exactly why the patches are
separate commits — do not squash them.

For each ZC-xxx, answer explicitly:

- Does the bug still exist upstream? (read the current upstream code, do not
  guess from the changelog)
- Did upstream fix it? → **drop our commit**, note it in the report.
- Did upstream refactor around it? → adapt, and update the
  `ZOUGCLOUD(ZC-xxx)` comment so it still explains *why*.

---

## Step 8 — Find patches upstream has fixed

For each patch in `docs/ZOUGCLOUD-PATCHES.md`, read the upstream version of the
files it lists and check whether the defect is gone. Useful probes:

```bash
git grep -n "single_instance::init" upstream/develop -- desktop/src-tauri/src/lib.rs   # ZC-001
git diff <our-base> upstream/develop -- desktop/src-tauri/src/updates.rs               # ZC-002
git diff <our-base> upstream/develop -- desktop/src-tauri/process/src/parser.rs        # ZC-003
git grep -n -i "steam" upstream/develop -- desktop/src-tauri/Cargo.toml                # ZC-004/005/006
git grep -n -i "playtime\|play_time" upstream/develop -- desktop/src-tauri desktop/main # ZC-008/009
```

ZC-004 through ZC-009 are **additions**, not fixes to upstream defects, so they
are not "fixed upstream" in the usual sense. What retires them is upstream
shipping an equivalent feature — the greps above are how you notice that. If it
happens, prefer upstream's version and delete ours.

**ZC-007 is `abandoned`, not pending.** Do not re-investigate GOG Galaxy unless
GOG has actually granted a Drop/ZougCloud Platform ID; the reasoning is in
`docs/ZOUGCLOUD-PATCHES.md` and has not changed. **ZC-010 must not be created** —
the Steam icon problem was a launch-command misconfiguration, not a code defect.

**Removing a patch upstream has fixed is the point of this exercise.** A fork
that only grows is a fork that eventually cannot be rebased.

---

## Step 9 — Remove what is no longer needed

Delete the commit, delete the `ZOUGCLOUD(ZC-xxx)` comments, delete the tests
that only guarded our version, and mark the entry `upstream fixed` in
`docs/ZOUGCLOUD-PATCHES.md` — keep the entry as history, do not erase it.

---

## Step 10 — Confirm we are still client-only

```bash
powershell -ExecutionPolicy Bypass -File scripts/verify-client-only.ps1 -Base upstream/develop
```

This must exit 0. If a rebase pulled a server file into one of our commits, fix
the commit — never the script.

---

## Step 11 — Test

```bash
cd desktop/src-tauri && cargo test -p process -p steam -p playtime --lib
cd desktop/src-tauri && cargo check
```

Then the manual plan: `docs/ZOUGCLOUD-TEST-PLAN.md`, in full, against the
**unchanged** RC5 server.

---

## Step 12 — Build the installer

```bash
pnpm install
cd desktop && pnpm tauri build
```

Output: `desktop/src-tauri/target/release/bundle/nsis/`.

For a guaranteed-clean build first remove `desktop/src-tauri/target`,
`desktop/.output`, `desktop/main/.output`.

Produce `SHA256SUMS.txt` and `BUILD-INFO.txt` recording the **new upstream base
commit**, the **ZougCloud commit**, date, Node/pnpm/Rust/Tauri versions,
architecture, SHA-256 and the exact build command.

---

## Step 13 — Update the docs, then report

Update in `docs/ZOUGCLOUD-FORK.md`: the baseline commit, and the server
compatibility statement if it changed.
Update in `docs/ZOUGCLOUD-PATCHES.md`: every patch state and commit hash.

Then produce the report. **The table is mandatory:**

```
UPSTREAM UPDATE REPORT
  previous base : <commit> (<date>)
  new base      : <commit> (<tag>, <date>)
  server        : ghcr.io/drop-oss/drop:<tag> — UNCHANGED
  commits       : <n> upstream commits absorbed

API COMPATIBILITY
  desktop API layer diff : <empty | summary>
  new endpoints called   : <none | list>
  verdict                : COMPATIBLE / INCOMPATIBLE — <why>

AUTO-UPDATER
  present upstream : yes/no — <action taken>

PATCH STATUS
  ZC-001  still required
  ZC-002  fixed upstream, patch removed
  ZC-003  conflict, review needed — <what to look at>
  ZC-007  abandoned (no GOG Platform ID) — do not reopen
  ZC-008  still required
  ZC-009  still required
  ZC-010  must not be created
  ZC-004  still required
  ...

TESTS
  automated : <pass/fail>
  manual    : <which ran, results>

INSTALLER
  path    : <path>
  sha-256 : <hash>
```

If the verdict is **INCOMPATIBLE**, stop there: no rebase, no build. Report
which feature breaks, why it cannot be solved client-side, and what server
change would be required. The decision to move the server is the fork owner's
alone.
