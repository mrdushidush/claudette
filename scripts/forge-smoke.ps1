<#
.SYNOPSIS
  Smoke-tests claudette forge-mode end-to-end on greenfield + brownfield
  missions, exercising the two newly-integrated Beast modules:
    * apply_diff   (fuzzy before/after edit tool)
    * agentic Planner (read-only investigation -> RELEVANT FILES + PLAN brief)

.DESCRIPTION
  For each mission this script:
    1. Creates an isolated throwaway git repo under -WorkRoot.
    2. Seeds it (empty README for greenfield; a multi-file repo with a planted
       bug for brownfield).
    3. Runs `claudette --forge "<prompt>"` with CLAUDETTE_FORGE_AUTO_APPROVE=1
       so the pipeline runs unattended (no [y/N] prompts). stderr is captured
       to a per-mission log.
    4. Parses the log for: planner localization ("RELEVANT FILES"), apply_diff
       usage, fix-loop rounds, and the final verifier score.
    5. Verifies the result — brownfield: runs the repo's test (python/node) and
       checks it now passes; greenfield: checks the expected artifact files
       exist and contain sanity markers.
    6. Prints a summary table and the artifact/log paths.

  REQUIREMENTS
    * A model backend reachable at -OllamaHost. Two modes:
        - LM Studio (default): -OllamaHost http://localhost:1234 with
          -OpenAICompat (default ON) — claudette POSTs to /v1/chat/completions.
          The chosen -Model must be a model id from LM Studio's list (JIT-loads
          on first request).
        - Native Ollama: -OllamaHost http://localhost:11434 -OpenAICompat:$false
          with the model pulled (`ollama pull <model>`).
    * Optional: `python` and/or `node` on PATH for brownfield verification
      (missions are skipped-as-unverified if the toolchain is absent).

  SAFETY
    CLAUDETTE_FORGE_AUTO_APPROVE=1 lets the model run bash/git/apply_diff WITHOUT
    confirmation. This script confines every run to a throwaway repo under
    -WorkRoot (default $env:TEMP\forge-smoke). Do not point -WorkRoot at a real
    project.

.EXAMPLE
  pwsh -File scripts\forge-smoke.ps1 -Model qwen3-coder:30b
  pwsh -File scripts\forge-smoke.ps1 -Only py-pricing,js-textutils -FixRounds 3
#>
[CmdletBinding()]
param(
    # Model used for ALL forge roles (Planner / Coder / Verifier). Override per
    # role with the CLAUDETTES_FORGE_*_MODEL env vars before running if desired.
    # Must match a model id served by the backend (LM Studio `/v1/models`).
    [string]$Model = $env:FORGE_SMOKE_MODEL,
    [string]$OllamaHost = $(if ($env:OLLAMA_HOST) { $env:OLLAMA_HOST } else { 'http://localhost:1234' }),
    # OpenAI-compat mode (LM Studio /v1/chat/completions). Default ON for the
    # :1234 LM Studio setup; pass -OpenAICompat:$false for native Ollama.
    [bool]$OpenAICompat = $true,
    [int]$FixRounds = 2,
    [int]$NumCtx = 16384,
    [string]$WorkRoot = (Join-Path $env:TEMP 'forge-smoke'),
    [string[]]$Only,            # run only these mission names
    [switch]$SkipBuild,         # use an already-built release binary
    [switch]$SeedOnly           # seed + verify only, skip the forge run (fixture check)
)

$ErrorActionPreference = 'Stop'
if (-not $Model) { $Model = if ($OpenAICompat) { 'qwen3-coder-30b-a3b-instruct' } else { 'qwen3-coder:30b' } }

$RepoRoot = Split-Path -Parent $PSScriptRoot
$Bin = Join-Path $RepoRoot 'target\release\claudette.exe'

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------
if (-not $SkipBuild -and -not $SeedOnly) {
    Write-Host "==> Building release binary..." -ForegroundColor Cyan
    Push-Location $RepoRoot
    try { cargo build --release -p claudette } finally { Pop-Location }
}
if (-not $SeedOnly -and -not (Test-Path $Bin)) { throw "claudette.exe not found at $Bin (build failed or -SkipBuild with no binary)" }

# ---------------------------------------------------------------------------
# Forge environment (applies to every mission via inherited process env)
# ---------------------------------------------------------------------------
$env:CLAUDETTE_FORGE_AUTO_APPROVE = '1'        # unattended: no [y/N] prompts
$env:CLAUDETTE_FACELESS          = '1'         # skip persona overlay noise
$env:OLLAMA_HOST                 = $OllamaHost
if ($OpenAICompat) { $env:CLAUDETTE_OPENAI_COMPAT = '1' }  # LM Studio /v1/chat/completions
else { Remove-Item Env:\CLAUDETTE_OPENAI_COMPAT -ErrorAction SilentlyContinue }
$env:CLAUDETTE_MAX_FIX_ROUNDS    = "$FixRounds"
$env:CLAUDETTE_MODEL             = $Model
$env:CLAUDETTE_CODER_MODEL       = $Model
$env:CLAUDETTE_NUM_CTX           = "$NumCtx"
$env:CLAUDETTE_CODER_NUM_CTX     = "$NumCtx"
$env:CLAUDETTES_FORGE_PLANNER_MODEL  = $Model
$env:CLAUDETTES_FORGE_CODER_MODEL    = $Model
$env:CLAUDETTES_FORGE_VERIFIER_MODEL = $Model

New-Item -ItemType Directory -Force -Path $WorkRoot | Out-Null

# ---------------------------------------------------------------------------
# Seed writers
# ---------------------------------------------------------------------------
function Write-File($Path, $Content) {
    $dir = Split-Path -Parent $Path
    if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
    # -NoNewline + explicit \n already embedded keeps LF endings; utf8 no-BOM.
    [System.IO.File]::WriteAllText($Path, $Content)
}

function Seed-Greenfield($dir, $name) {
    Write-File (Join-Path $dir 'README.md') "# $name`n`nScaffold for a forge smoke-test mission.`n"
    Write-File (Join-Path $dir '.gitignore') "node_modules/`n__pycache__/`n"
}

function Seed-PyPricing($dir) {
    Write-File (Join-Path $dir 'store/__init__.py') ""
    Write-File (Join-Path $dir 'store/inventory.py') @'
"""In-memory product catalog (correct — do not change)."""

CATALOG = {
    "espresso": 100.0,
    "latte": 50.0,
}


def price_of(sku):
    return CATALOG[sku]
'@
    Write-File (Join-Path $dir 'store/pricing.py') @'
"""Pricing helpers."""


def apply_discount(price, pct):
    # BUG: returns the discount amount, not the discounted price.
    return price * (pct / 100)
'@
    Write-File (Join-Path $dir 'test_pricing.py') @'
"""Run with: python test_pricing.py  (exit 0 = pass)."""
from store.pricing import apply_discount

assert apply_discount(100, 20) == 80.0, f"100 less 20 percent should be 80.0, got {apply_discount(100, 20)}"
assert apply_discount(50, 10) == 45.0, f"50 less 10 percent should be 45.0, got {apply_discount(50, 10)}"
print("ok")
'@
}

function Seed-JsTextutils($dir) {
    Write-File (Join-Path $dir 'src/format.js') @'
// Correct — do not change.
function titleCase(s) {
  return s.replace(/\b\w/g, (c) => c.toUpperCase());
}
module.exports = { titleCase };
'@
    Write-File (Join-Path $dir 'src/slug.js') @'
// BUG: does not lowercase, does not strip punctuation, does not collapse
// repeated separators.
function slugify(s) {
  return s.trim().replace(/ /g, "-");
}
module.exports = { slugify };
'@
    Write-File (Join-Path $dir 'src/index.js') @'
module.exports = {
  ...require("./slug"),
  ...require("./format"),
};
'@
    Write-File (Join-Path $dir 'test.js') @'
const assert = require("assert");
const { slugify } = require("./src/slug");

assert.strictEqual(slugify("Hello   World!"), "hello-world");
assert.strictEqual(slugify("  Foo_Bar  Baz "), "foo-bar-baz");
console.log("ok");
'@
}

function Seed-PyTemp($dir) {
    Write-File (Join-Path $dir 'lib/__init__.py') ""
    Write-File (Join-Path $dir 'lib/distance.py') @'
"""Distance conversions (correct — do not change)."""

def km_to_miles(km):
    return km * 0.621371
'@
    Write-File (Join-Path $dir 'lib/temperature.py') @'
"""Temperature conversions."""


def c_to_f(c):
    # BUG: forgot the + 32 offset.
    return c * 9 / 5
'@
    Write-File (Join-Path $dir 'test_temperature.py') @'
"""Run with: python test_temperature.py  (exit 0 = pass)."""
from lib.temperature import c_to_f

assert c_to_f(100) == 212, f"100C should be 212F, got {c_to_f(100)}"
assert c_to_f(0) == 32, f"0C should be 32F, got {c_to_f(0)}"
print("ok")
'@
}

# ---------------------------------------------------------------------------
# Verifiers — return @{ ok = $bool; note = "..." }
# ---------------------------------------------------------------------------
function Have($exe) { [bool](Get-Command $exe -ErrorAction SilentlyContinue) }

# Run git with stderr merged + discarded so harmless warnings (e.g. CRLF
# conversion) don't trip $ErrorActionPreference='Stop' (PS 5.1 wraps native
# stderr as a terminating NativeCommandError). Throws only on nonzero exit.
function Invoke-Git {
    param([Parameter(ValueFromRemainingArguments = $true)][string[]]$GitArgs)
    $old = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try { & git @GitArgs 2>&1 | Out-Null; $code = $LASTEXITCODE }
    finally { $ErrorActionPreference = $old }
    if ($code -ne 0) { throw "git $($GitArgs -join ' ') failed (exit $code)" }
}

# Run a native test command, capturing all output and the exit code without
# letting native stderr trip $ErrorActionPreference='Stop'. Returns the result
# shape @{ ok; note } where ok=$null means "couldn't verify".
function Run-Cmd($dir, $exe, $cmdArgs, $missing) {
    if (-not (Have $exe)) { return @{ ok = $null; note = $missing } }
    Push-Location $dir
    $old = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
    try {
        $out = & $exe @cmdArgs 2>&1 | Out-String
        $ok = ($LASTEXITCODE -eq 0)
        $last = ($out.Trim() -split "`n" | Where-Object { $_.Trim() } | Select-Object -Last 1)
        return @{ ok = $ok; note = $last }
    } finally { $ErrorActionPreference = $old; Pop-Location }
}

function Run-PyTest($dir, $file) { Run-Cmd $dir 'python' @($file) 'python not on PATH (unverified)' }
function Run-NodeTest($dir)      { Run-Cmd $dir 'node'   @('test.js') 'node not on PATH (unverified)' }

function Verify-Files($dir, $files, $markers) {
    foreach ($f in $files) {
        if (-not (Test-Path (Join-Path $dir $f))) { return @{ ok = $false; note = "missing $f" } }
    }
    $joined = ($files | ForEach-Object { Get-Content (Join-Path $dir $_) -Raw }) -join "`n"
    foreach ($m in $markers) {
        if ($joined -notmatch $m) { return @{ ok = $false; note = "marker /$m/ not found" } }
    }
    return @{ ok = $true; note = "files + markers present" }
}

# ---------------------------------------------------------------------------
# Mission definitions
# ---------------------------------------------------------------------------
$Missions = @(
    @{
        name = 'space-invaders'; type = 'greenfield'
        prompt = 'Create a single-file browser game: a playable Space Invaders in index.html using an HTML5 <canvas> and vanilla JavaScript only (no external libraries or CDNs). The player ship moves left/right with the arrow keys and fires with the spacebar; a grid of invaders descends and the game ends on collision or when all invaders are destroyed. Put all HTML, CSS, and JavaScript inside index.html.'
        seed = { param($d) Seed-Greenfield $d 'space-invaders' }
        verify = { param($d) Verify-Files $d @('index.html') @('<canvas', '(?i)invader', '(?i)keydown|ArrowLeft') }
    },
    @{
        name = 'storefront-landing'; type = 'greenfield'
        prompt = "Build a modern storefront landing page for a fictional coffee brand called 'Brew & Co'. Create index.html and styles.css. Include: a hero section with a headline and a 'Shop now' call-to-action button, a responsive product grid of 3 featured products each with a name and price, and a footer. Put all styling in styles.css with no CSS frameworks."
        seed = { param($d) Seed-Greenfield $d 'storefront-landing' }
        verify = { param($d) Verify-Files $d @('index.html', 'styles.css') @('(?i)brew', '(?i)shop now', '(?i)\$\d') }
    },
    @{
        name = 'py-pricing'; type = 'brownfield'
        prompt = 'Running `python test_pricing.py` fails: apply_discount in store/pricing.py returns the wrong value (it returns the discount amount instead of the discounted price). Find and fix the bug so the test passes. Do not edit the test.'
        seed = { param($d) Seed-PyPricing $d }
        verify = { param($d) Run-PyTest $d 'test_pricing.py' }
    },
    @{
        name = 'js-textutils'; type = 'brownfield'
        prompt = 'Running `node test.js` fails: slugify in src/slug.js does not lowercase the input, strip punctuation, or collapse repeated separators. Fix slugify so the assertions in test.js pass. Do not edit test.js.'
        seed = { param($d) Seed-JsTextutils $d }
        verify = { param($d) Run-NodeTest $d }
    },
    @{
        name = 'py-temp'; type = 'brownfield'
        prompt = 'Running `python test_temperature.py` fails: c_to_f in lib/temperature.py is missing the +32 offset of the Celsius-to-Fahrenheit formula. Find and fix the bug so the test passes. Do not change the test.'
        seed = { param($d) Seed-PyTemp $d }
        verify = { param($d) Run-PyTest $d 'test_temperature.py' }
    }
)

if ($Only) { $Missions = $Missions | Where-Object { $Only -contains $_.name } }

# ---------------------------------------------------------------------------
# Runner
# ---------------------------------------------------------------------------
$results = @()
foreach ($m in $Missions) {
    $name = $m.name
    $dir = Join-Path $WorkRoot $name
    $log = Join-Path $WorkRoot "$name.log"
    Write-Host "`n=== [$($m.type)] $name ===" -ForegroundColor Cyan

    if (Test-Path $dir) { Remove-Item -Recurse -Force $dir }
    New-Item -ItemType Directory -Force -Path $dir | Out-Null

    & $m.seed $dir
    Push-Location $dir
    try {
        Invoke-Git init -q
        Invoke-Git config core.autocrlf false
        Invoke-Git add -A
        Invoke-Git -c user.email=smoke@forge -c user.name=smoke commit -q -m "seed: $name"
    } finally { Pop-Location }

    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $exit = $null
    if ($SeedOnly) {
        Write-Host "    -SeedOnly: skipping forge run (fixture check)" -ForegroundColor DarkGray
    } else {
        Write-Host "    running forge (model=$Model, fix-rounds=$FixRounds)..." -ForegroundColor DarkGray
        $p = Start-Process -FilePath $Bin -ArgumentList @('--forge', $m.prompt) `
            -WorkingDirectory $dir -NoNewWindow -Wait -PassThru `
            -RedirectStandardError $log -RedirectStandardOutput "$log.out"
        $exit = $p.ExitCode
    }
    $sw.Stop()

    $logText = if (Test-Path $log) { Get-Content $log -Raw } else { '' }
    # Strip ANSI color codes so token matching is reliable.
    $logText = [regex]::Replace($logText, "\x1b\[[0-9;?]*[A-Za-z]", "")
    # Planner localized? Its (free-form) brief, captured between the planner and
    # coder phase banners, should name a source file (path-with-extension).
    $plannerBlock = [regex]::Match($logText, '(?s)forge: planner(.*?)forge: (coder|verifier|submit)').Groups[1].Value
    $localized = [bool]($plannerBlock -match '[\w./\\-]+\.(py|js|ts|jsx|tsx|html|css|rs|go|java|rb)')
    # apply_diff logs a "apply_diff: <path>" line on every call (see fuzzy_apply.rs).
    $usedDiff  = [bool]($logText -match 'apply_diff:')
    $rounds    = ([regex]::Matches($logText, '(?i)forge: coder \(round')).Count
    $scoreM    = [regex]::Match($logText, 'score=(\d+)\s+pass=(true|false)')
    $score     = if ($scoreM.Success) { "$($scoreM.Groups[1].Value)/$($scoreM.Groups[2].Value)" } else { 'n/a' }

    $v = & $m.verify $dir

    $results += [pscustomobject]@{
        Mission   = $name
        Type      = $m.type
        Verified  = $(if ($null -eq $v.ok) { 'SKIP' } elseif ($v.ok) { 'PASS' } else { 'FAIL' })
        Score     = $score
        Rounds    = $rounds
        Localized = $(if ($localized) { 'yes' } else { '-' })
        ApplyDiff = $(if ($usedDiff) { 'yes' } else { '-' })
        Secs      = [int]$sw.Elapsed.TotalSeconds
        Note      = $v.note
        Exit      = $exit
        Dir       = $dir
        Log       = $log
    }
}

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
Write-Host "`n================== FORGE SMOKE SUMMARY ==================" -ForegroundColor Green
$results | Format-Table Mission, Type, Verified, Score, Rounds, Localized, ApplyDiff, Secs -AutoSize
Write-Host "Notes:" -ForegroundColor DarkGray
$results | ForEach-Object { Write-Host ("  {0,-18} {1}" -f $_.Mission, $_.Note) -ForegroundColor DarkGray }
Write-Host "`nArtifacts under: $WorkRoot" -ForegroundColor DarkGray
Write-Host "  (open greenfield index.html in a browser to eyeball; per-mission logs are <name>.log)" -ForegroundColor DarkGray

$failed = @($results | Where-Object { $_.Verified -eq 'FAIL' }).Count
if ($failed -gt 0) { Write-Host "`n$failed mission(s) FAILED verification." -ForegroundColor Yellow; exit 1 }
Write-Host "`nAll verified missions passed (SKIP = no toolchain to check)." -ForegroundColor Green
