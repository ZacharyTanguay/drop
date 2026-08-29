<#
.SYNOPSIS
    Enforces the ZougCloud fork's absolute client-only rule.

.DESCRIPTION
    The ZougCloud fork exists so that a Windows Desktop client can be improved
    while the Drop server stays byte-for-byte upstream (ghcr.io/drop-oss/drop).
    A single stray edit under server/ or backend/ would silently turn this into
    a client+server fork and destroy that guarantee -- so the rule is enforced
    mechanically rather than by discipline.

    The policy is DEFAULT-DENY: a path is refused unless it matches the
    allow-list below. That way a new upstream top-level directory is refused on
    sight instead of quietly slipping through.

    See docs/ZOUGCLOUD-FORK.md for the rationale.

.PARAMETER Base
    Ref our changes are measured against. Defaults to upstream/develop.
    The comparison uses the merge base, so upstream commits we have merged in
    are never reported as ours.

.PARAMETER IncludeWorkingTree
    Also inspect uncommitted changes. Use this in a pre-commit hook.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts/verify-client-only.ps1

.EXAMPLE
    ./scripts/verify-client-only.ps1 -Base upstream/main -IncludeWorkingTree
#>
[CmdletBinding()]
param(
    [string] $Base = 'upstream/develop',
    [switch] $IncludeWorkingTree
)

$ErrorActionPreference = 'Stop'

# ---------------------------------------------------------------------------
# Policy
# ---------------------------------------------------------------------------

# Everything ZougCloud is allowed to touch.
$Allowed = @(
    'desktop/'                              # the client -- our entire remit
    'docs/ZOUGCLOUD-'                       # fork docs (FORK / PATCHES / TEST-PLAN)
    'docs/UPSTREAM-UPDATE-PROCEDURE.md'     # upstream sync runbook
    'scripts/'                              # our build and guard scripts
    '.github/workflows/zougcloud-'          # our own CI, namespaced
)

# Paths that would make this a server fork. Listed separately from the
# default-deny so the failure message can say *why* it matters.
$ServerCritical = @{
    'server/'            = 'the Drop server itself'
    'backend/'           = 'the Go backend'
    'torrential/'        = 'the torrent backend, shipped by the server'
    'cli/'               = 'the server-side CLI'
    'Dockerfile'         = 'the server container image'
    '.dockerignore'      = 'the server container build context'
    'sites/'             = 'the public websites'
    'libraries/base/'    = 'the Nuxt layer SHARED by desktop/main AND server -- editing it changes the server build'
    'pnpm-workspace.yaml' = 'the monorepo workspace definition'
    'pnpm-lock.yaml'     = 'the root lockfile, which pins server dependencies'
    'package.json'       = 'the root manifest'
}

# ---------------------------------------------------------------------------

function Get-ChangedFiles {
    param([string] $BaseRef, [bool] $WithWorkingTree)

    git rev-parse --verify --quiet "$BaseRef" > $null
    if ($LASTEXITCODE -ne 0) {
        throw "Base ref '$BaseRef' does not exist. Run 'git fetch upstream' first, or pass -Base."
    }

    # Three dots: compare against the merge base, so upstream work merged into
    # this branch is not mistaken for ours.
    $files = @(git diff --name-only "$BaseRef...HEAD")
    if ($LASTEXITCODE -ne 0) { throw 'git diff failed' }

    if ($WithWorkingTree) {
        $files += @(git diff --name-only HEAD)
        $files += @(git ls-files --others --exclude-standard)
    }

    $files | Where-Object { $_ } | Sort-Object -Unique
}

function Test-Allowed {
    param([string] $Path)
    foreach ($prefix in $Allowed) {
        if ($Path.StartsWith($prefix, [StringComparison]::Ordinal)) { return $true }
    }
    return $false
}

function Get-ServerReason {
    param([string] $Path)
    foreach ($prefix in $ServerCritical.Keys) {
        if ($Path.StartsWith($prefix, [StringComparison]::Ordinal)) {
            return $ServerCritical[$prefix]
        }
    }
    return $null
}

# ---------------------------------------------------------------------------

$repoRoot = (git rev-parse --show-toplevel)
if ($LASTEXITCODE -ne 0) { throw 'Not inside a git repository.' }
Set-Location $repoRoot

$changed = Get-ChangedFiles -BaseRef $Base -WithWorkingTree $IncludeWorkingTree.IsPresent

Write-Host ''
Write-Host 'ZougCloud client-only check' -ForegroundColor Cyan
Write-Host "  base      : $Base"
Write-Host "  files      : $($changed.Count) changed"
if ($IncludeWorkingTree) { Write-Host '  scope     : commits + working tree' }
else { Write-Host '  scope     : commits only' }
Write-Host ''

if ($changed.Count -eq 0) {
    Write-Host 'No divergence from upstream. PASS.' -ForegroundColor Green
    exit 0
}

$violations = @()
foreach ($file in $changed) {
    if (Test-Allowed -Path $file) { continue }

    $reason = Get-ServerReason -Path $file
    if (-not $reason) { $reason = 'outside the client-only allow-list' }
    $violations += [pscustomobject]@{ File = $file; Reason = $reason }
}

$okCount = $changed.Count - $violations.Count
Write-Host "  $okCount file(s) within the client-only boundary" -ForegroundColor Green

if ($violations.Count -eq 0) {
    Write-Host ''
    Write-Host 'PASS - the fork is still client-only.' -ForegroundColor Green
    exit 0
}

Write-Host ''
Write-Host "FAIL - $($violations.Count) file(s) break the client-only rule:" -ForegroundColor Red
foreach ($v in $violations) {
    Write-Host ("  {0}" -f $v.File) -ForegroundColor Red
    Write-Host ("      {0}" -f $v.Reason) -ForegroundColor DarkYellow
}

Write-Host ''
Write-Host 'The ZougCloud fork must run against an unmodified upstream server' -ForegroundColor Yellow
Write-Host '(ghcr.io/drop-oss/drop). Touching the paths above means our client can' -ForegroundColor Yellow
Write-Host 'no longer be handed to a stock Drop server, which is the whole point' -ForegroundColor Yellow
Write-Host 'of the fork. Solve the problem inside desktop/ instead, or -- if it is' -ForegroundColor Yellow
Write-Host 'genuinely impossible -- take it upstream as a PR to Drop-OSS/drop.' -ForegroundColor Yellow
Write-Host ''
Write-Host 'See docs/ZOUGCLOUD-FORK.md, section "The client-only rule".' -ForegroundColor Yellow
Write-Host ''

exit 1
