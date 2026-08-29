<#
.SYNOPSIS
    Packages a built ZougCloud Desktop installer for distribution.

.DESCRIPTION
    Takes the NSIS installer that `pnpm tauri build` produced and turns it into
    something we can hand to members and still identify a year later: renamed to
    a ZougCloud filename, hashed, and accompanied by a BUILD-INFO.txt recording
    the exact provenance of the build.

    The provenance matters twice over. It lets us answer "which build is this?"
    from a bug report, and it satisfies AGPL-3.0 s6: because we convey a modified
    binary, we must be able to point at the corresponding source. The recorded
    ZougCloud commit is that pointer.

    Run this AFTER building. It does not build anything itself, so the artefact
    you ship is exactly the one you tested.

.PARAMETER OutputDir
    Where to write the distributable set.

.PARAMETER SkipDirtyCheck
    Package even though the working tree has uncommitted changes. Refuses by
    default: an installer built from an unrecorded tree cannot be reproduced,
    and its BUILD-INFO.txt would be a lie.

.EXAMPLE
    ./scripts/package-release.ps1
#>
[CmdletBinding()]
param(
    [string] $OutputDir = "$env:USERPROFILE\Documents\Drop-ZougCloud-Build",
    [switch] $SkipDirtyCheck
)

$ErrorActionPreference = 'Stop'

$repoRoot = (git rev-parse --show-toplevel)
if ($LASTEXITCODE -ne 0) { throw 'Not inside a git repository.' }
Set-Location $repoRoot

# --- provenance ------------------------------------------------------------

$dirty = @(git status --porcelain)
if ($dirty.Count -gt 0 -and -not $SkipDirtyCheck) {
    Write-Host 'Working tree is not clean:' -ForegroundColor Red
    $dirty | ForEach-Object { Write-Host "  $_" -ForegroundColor Red }
    throw 'Refusing to package an unreproducible build. Commit first, or pass -SkipDirtyCheck.'
}

$zcCommit = (git rev-parse HEAD).Trim()
$zcBranch = (git rev-parse --abbrev-ref HEAD).Trim()

git rev-parse --verify --quiet upstream/develop > $null
if ($LASTEXITCODE -eq 0) {
    $upstreamBase = (git merge-base HEAD upstream/develop).Trim()
    $upstreamDate = (git log -1 --format='%ci' $upstreamBase).Trim()
} else {
    $upstreamBase = '<unknown - run: git fetch upstream>'
    $upstreamDate = '<unknown>'
}

$patches = @(git log --oneline "$upstreamBase..HEAD" --reverse)

# --- artefact --------------------------------------------------------------

$nsisDir = Join-Path $repoRoot 'desktop\src-tauri\target\release\bundle\nsis'
if (-not (Test-Path $nsisDir)) { throw "No NSIS output at $nsisDir. Build first: cd desktop; pnpm tauri build" }

# Newest installer wins, so a stale bundle from an earlier version is not shipped.
$installer = Get-ChildItem "$nsisDir\*-setup.exe" | Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $installer) { throw "No *-setup.exe in $nsisDir." }

$conf = Get-Content (Join-Path $repoRoot 'desktop\src-tauri\tauri.conf.json') -Raw | ConvertFrom-Json
$version = $conf.version

$appExe = Join-Path $repoRoot 'desktop\src-tauri\target\release\drop-app.exe'

New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
$shippedName = "Drop-ZougCloud_${version}_x64-setup.exe"
$shipped = Join-Path $OutputDir $shippedName
Copy-Item $installer.FullName -Destination $shipped -Force

$hash = (Get-FileHash $shipped -Algorithm SHA256).Hash
$appHash = if (Test-Path $appExe) { (Get-FileHash $appExe -Algorithm SHA256).Hash } else { 'n/a' }
$appSize = if (Test-Path $appExe) { (Get-Item $appExe).Length } else { 0 }

# --- toolchain -------------------------------------------------------------

function Try-Version([scriptblock] $cmd) {
    try { (& $cmd 2>&1 | Select-Object -First 1).ToString().Trim() } catch { 'not found' }
}

$nodeV  = Try-Version { node -v }
$pnpmV  = Try-Version { pnpm -v }
$rustcV = Try-Version { rustc -V }
$cargoV = Try-Version { cargo -V }

$lock = Join-Path $repoRoot 'desktop\src-tauri\Cargo.lock'
$tauriCrate = 'unknown'
if (Test-Path $lock) {
    $lines = Get-Content $lock
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -eq 'name = "tauri"') { $tauriCrate = ($lines[$i + 1] -replace '.*"(.*)".*', '$1'); break }
    }
}

# --- write -----------------------------------------------------------------

"$hash *$shippedName" | Out-File (Join-Path $OutputDir 'SHA256SUMS.txt') -Encoding ascii

$info = @"
================================================================
 DROP DESKTOP CLIENT - ZOUGCLOUD BUILD
================================================================

VERSION
-------
$version

PROVENANCE
----------
ZougCloud commit : $zcCommit
ZougCloud branch : $zcBranch
Upstream base    : $upstreamBase
Upstream date    : $upstreamDate
Fork remote      : $(git remote get-url origin 2>`$null)
Upstream remote  : $(git remote get-url upstream 2>`$null)
Working tree     : $(if ($dirty.Count -eq 0) { 'clean' } else { "DIRTY ($($dirty.Count) file(s)) - NOT REPRODUCIBLE" })

Target server    : ghcr.io/drop-oss/drop:0.4.0-rc-5
                   (commit 6f7471869515e3a61121ae2af2d556d8914d30e4)
                   The client is compatible with this server: the Desktop API
                   layer is byte-identical to that tag. See docs/ZOUGCLOUD-FORK.md.

ZOUGCLOUD PATCHES IN THIS BUILD
-------------------------------
$($patches -join "`n")

See docs/ZOUGCLOUD-PATCHES.md for the full inventory.

BUILD
-----
Date         : $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss K')
Architecture : x64 (AMD64)
                 The installer itself is a 32-bit NSIS stub carrying an x64
                 payload -- that is how NSIS always works.

ARTEFACTS
---------
$shippedName
  Size    : $('{0:N0}' -f (Get-Item $shipped).Length) bytes
  SHA-256 : $hash

drop-app.exe (payload, for reference)
  Size    : $('{0:N0}' -f $appSize) bytes
  SHA-256 : $appHash

No MSI is produced: tauri.conf.json sets "wix": null and bundle.targets to
["nsis","deb","rpm","dmg"]. That is upstream configuration; we do not change it.

TOOLCHAIN
---------
Node.js     : $nodeV
pnpm        : $pnpmV
rustc       : $rustcV
cargo       : $cargoV
tauri crate : $tauriCrate

NOTE: desktop/src-tauri/rust-toolchain.toml pins "nightly" WITHOUT a date, so
a rebuild on another day uses a different compiler and will not be bit-for-bit
identical. The source is exact; the binary is not reproducible byte-wise. This
is upstream's configuration.

BUILD COMMAND
-------------
  git checkout $zcCommit
  pnpm install
  cd desktop
  pnpm tauri build          # env: NO_STRIP=true

For a guaranteed-clean rebuild, first remove:
  desktop/src-tauri/target, desktop/.output, desktop/main/.output

INSTALLING
----------
Upgrades an existing "Drop Desktop Client" install in place (productName and
identifier are deliberately unchanged from upstream).

%APPDATA%\drop -- games, database, config -- is preserved. The Tauri NSIS
uninstaller only ever removes %APPDATA%\org.droposs.client and
%LOCALAPPDATA%\org.droposs.client, and only when the "delete application data"
box is ticked AND it is not an upgrade. Drop's real data directory is never a
target.

LICENCE
-------
GNU Affero General Public License v3 (desktop/LICENSE, (C) 2024 DecDuck).

This binary is a MODIFIED version of Drop Desktop. Under AGPL s6, anyone given
this installer is entitled to the corresponding source, which is commit
$zcCommit
at $(git remote get-url origin 2>`$null)

Upstream: https://github.com/Drop-OSS/drop
================================================================
"@

$info | Out-File (Join-Path $OutputDir 'BUILD-INFO.txt') -Encoding utf8

Write-Host ''
Write-Host 'Packaged.' -ForegroundColor Green
Write-Host "  $shipped"
Write-Host "  SHA-256 : $hash"
Write-Host "  version : $version"
Write-Host "  commit  : $zcCommit"
Write-Host ''
Write-Host "Wrote BUILD-INFO.txt and SHA256SUMS.txt to $OutputDir" -ForegroundColor Green
