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
    [ValidateSet('smoke', 'stress', 'all')]
    [string]$Set = 'smoke',     # which mission set: smoke (5) | stress (20) | all (25)
    [string[]]$Only,            # run only these mission names
    [switch]$SkipBuild,         # use an already-built release binary
    [switch]$SeedOnly,          # seed + verify only, skip the forge run (fixture check)
    [switch]$SecurityReview     # enable the opt-in forge security-review stage
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
if ($SecurityReview) { $env:CLAUDETTE_FORGE_SECURITY_REVIEW = '1' }
else { Remove-Item Env:\CLAUDETTE_FORGE_SECURITY_REVIEW -ErrorAction SilentlyContinue }
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
# Stress set (-Set stress): 11 brownfield + 3 multi-file refactor + 6 greenfield.
# Each seed writes a multi-file repo (bug in one file, plus a decoy) so the
# agentic Planner has to localize. Helper keeps the bodies compact.
# ---------------------------------------------------------------------------
function Seed-Files($dir, $files) {
    foreach ($k in $files.Keys) { Write-File (Join-Path $dir $k) $files[$k] }
}

# --- brownfield (11) ---
function Seed-BfOffbyone($d) { Seed-Files $d @{
  'pkg/__init__.py' = ''
  'pkg/textutil.py' = "def shout(s):`n    return s.upper() + '!'`n"
  'pkg/slicer.py' = @'
"""List slicing helpers."""


def last_n(items, n):
    # BUG: off by one — drops one element.
    return items[-n + 1:]
'@
  'test_slicer.py' = @'
from pkg.slicer import last_n

assert last_n([1, 2, 3, 4, 5], 2) == [4, 5], f"got {last_n([1, 2, 3, 4, 5], 2)}"
assert last_n([1, 2, 3], 3) == [1, 2, 3], f"got {last_n([1, 2, 3], 3)}"
print("ok")
'@
} }

function Seed-BfOperator($d) { Seed-Files $d @{
  'pkg/__init__.py' = ''
  'pkg/fmt.py' = "def pct(x):`n    return f'{x}%'`n"
  'pkg/grades.py' = @'
"""Grading rules."""


def is_passing(score):
    # BUG: passing should be 60 or above.
    return score > 60
'@
  'test_grades.py' = @'
from pkg.grades import is_passing

assert is_passing(60) is True, "60 must pass"
assert is_passing(59) is False, "59 must fail"
assert is_passing(100) is True
print("ok")
'@
} }

function Seed-BfMutableDefault($d) { Seed-Files $d @{
  'pkg/__init__.py' = ''
  'pkg/ioutil.py' = "def label(x):`n    return f'[{x}]'`n"
  'pkg/collector.py' = @'
"""Item collector."""


def add_item(item, bag=[]):
    # BUG: mutable default arg is shared across calls.
    bag.append(item)
    return bag
'@
  'test_collector.py' = @'
from pkg.collector import add_item

assert add_item("a") == ["a"], f"got {add_item('a')}"
assert add_item("b") == ["b"], "mutable default leaked between calls"
print("ok")
'@
} }

function Seed-BfRegex($d) { Seed-Files $d @{
  'src/util.js' = "module.exports = { trimmed: (s) => s.trim() };`n"
  'src/validate.js' = @'
// BUG: regex is far too loose — no domain dot, no anchors.
function isEmail(s) {
  return /.+@.+/.test(s);
}
module.exports = { isEmail };
'@
  'test.js' = @'
const assert = require("assert");
const { isEmail } = require("./src/validate");

assert.strictEqual(isEmail("a@b.com"), true);
assert.strictEqual(isEmail("a@b"), false);
assert.strictEqual(isEmail("nope"), false);
console.log("ok");
'@
} }

function Seed-BfRecursion($d) { Seed-Files $d @{
  'pkg/__init__.py' = ''
  'pkg/seq.py' = "def head(xs):`n    return xs[0]`n"
  'pkg/math2.py' = @'
"""Recursive math."""


def factorial(n):
    # BUG: base case misses 0 (recurses forever / wrong).
    if n == 1:
        return 1
    return n * factorial(n - 1)
'@
  'test_math2.py' = @'
from pkg.math2 import factorial

assert factorial(0) == 1, f"0! must be 1, got {factorial(0)}"
assert factorial(5) == 120
print("ok")
'@
} }

function Seed-BfDate($d) { Seed-Files $d @{
  'pkg/__init__.py' = ''
  'pkg/calutil.py' = "def is_weekend(d):`n    return d.weekday() >= 5`n"
  'pkg/dates.py' = @'
"""Date math."""
from datetime import date  # noqa: F401


def days_between(a, b):
    # BUG: operands reversed — returns a negative count.
    return (a - b).days
'@
  'test_dates.py' = @'
from datetime import date
from pkg.dates import days_between

got = days_between(date(2020, 1, 1), date(2020, 1, 11))
assert got == 10, f"expected 10, got {got}"
print("ok")
'@
} }

function Seed-BfSort($d) { Seed-Files $d @{
  'src/index.js' = "module.exports = require('./sorting');`n"
  'src/sorting.js' = @'
// BUG: Array.prototype.sort defaults to lexicographic order.
function sortNums(arr) {
  return [...arr].sort();
}
module.exports = { sortNums };
'@
  'test.js' = @'
const assert = require("assert");
const { sortNums } = require("./src/sorting");

assert.deepStrictEqual(sortNums([10, 2, 1]), [1, 2, 10]);
assert.deepStrictEqual(sortNums([3, 30, 4]), [3, 4, 30]);
console.log("ok");
'@
} }

function Seed-BfNullGuard($d) { Seed-Files $d @{
  'src/defaults.js' = "module.exports = { DEFAULT_PORT: 8080 };`n"
  'src/config.js' = @'
// BUG: throws when cfg.server is missing; should fall back to 8080.
function getPort(cfg) {
  return cfg.server.port;
}
module.exports = { getPort };
'@
  'test.js' = @'
const assert = require("assert");
const { getPort } = require("./src/config");

assert.strictEqual(getPort({ server: { port: 3000 } }), 3000);
assert.strictEqual(getPort({}), 8080);
console.log("ok");
'@
} }

function Seed-BfAccumulator($d) { Seed-Files $d @{
  'pkg/__init__.py' = ''
  'pkg/agg.py' = "def total(xs):`n    return sum(xs)`n"
  'pkg/stats.py' = @'
"""Running statistics."""


def running_max(nums):
    # BUG: seeds the max at 0 — wrong for all-negative input.
    m = 0
    for n in nums:
        if n > m:
            m = n
    return m
'@
  'test_stats.py' = @'
from pkg.stats import running_max

assert running_max([-3, -1, -7]) == -1, f"got {running_max([-3, -1, -7])}"
assert running_max([2, 9, 4]) == 9
print("ok")
'@
} }

function Seed-BfBoundary($d) { Seed-Files $d @{
  'pkg/__init__.py' = ''
  'pkg/ranges.py' = "def span(lo, hi):`n    return hi - lo`n"
  'pkg/clampmod.py' = @'
"""Clamp a value into a range."""


def clamp(x, lo, hi):
    # BUG: returns None for in-range values (missing final return).
    if x < lo:
        return lo
    if x > hi:
        return hi
'@
  'test_clamp.py' = @'
from pkg.clampmod import clamp

assert clamp(5, 0, 10) == 5, f"in-range got {clamp(5, 0, 10)}"
assert clamp(-1, 0, 10) == 0
assert clamp(11, 0, 10) == 10
print("ok")
'@
} }

function Seed-BfString($d) { Seed-Files $d @{
  'src/case.js' = "module.exports = { cap: (s) => s.charAt(0).toUpperCase() + s.slice(1) };`n"
  'src/strings.js' = @'
// BUG: never truncates — returns the input unchanged.
function truncate(s, n) {
  return s;
}
module.exports = { truncate };
'@
  'test.js' = @'
const assert = require("assert");
const { truncate } = require("./src/strings");

// length <= n: returned unchanged. Otherwise: first (n-3) chars + "..." (three
// ASCII dots), so the result is exactly n characters.
assert.strictEqual(truncate("hello", 10), "hello");
assert.strictEqual(truncate("hello world", 8), "hello...");
assert.strictEqual(truncate("abcdefgh", 5), "ab...");
console.log("ok");
'@
} }

# --- multi-file refactor (3) ---
function Seed-RfRename($d) { Seed-Files $d @{
  'pkg/__init__.py' = ''
  'pkg/mathlib.py' = @'
"""Core math."""


def calc(a, b):
    return a + b
'@
  'pkg/report.py' = @'
from pkg.mathlib import calc


def make_report(nums):
    total = 0
    for n in nums:
        total = calc(total, n)
    return f"total={total}"
'@
  'test_api.py' = @'
"""Requires `calc` renamed to `compute` across pkg/ (lib + callers)."""
from pkg.mathlib import compute
from pkg.report import make_report

assert compute(2, 3) == 5
assert make_report([1, 2, 3]) == "total=6"
print("ok")
'@
} }

function Seed-RfExtract($d) { Seed-Files $d @{
  'src/a.js' = @'
function clamp(x) {
  return x < 0 ? 0 : x > 100 ? 100 : x;
}
function processA(v) {
  return clamp(v) + 1;
}
module.exports = { processA };
'@
  'src/b.js' = @'
function clamp(x) {
  return x < 0 ? 0 : x > 100 ? 100 : x;
}
function processB(v) {
  return clamp(v) * 2;
}
module.exports = { processB };
'@
  'test.js' = @'
// Requires `clamp` extracted into src/util.js (exported) and imported by a.js + b.js.
const assert = require("assert");
const { clamp } = require("./src/util");
const { processA } = require("./src/a");
const { processB } = require("./src/b");

assert.strictEqual(clamp(150), 100);
assert.strictEqual(processA(150), 101);
assert.strictEqual(processB(-5), 0);
console.log("ok");
'@
} }

function Seed-RfSignature($d) { Seed-Files $d @{
  'geo/__init__.py' = ''
  'geo/area.py' = @'
"""Area helpers."""


def rectangle_area(w, h):
    return w * h
'@
  'geo/shapes.py' = @'
from geo.area import rectangle_area


def total_area(rects):
    return sum(rectangle_area(w, h) for w, h in rects)
'@
  'test_area.py' = @'
"""rectangle_area must gain an optional `scale=1` factor; callers unaffected."""
from geo.area import rectangle_area
from geo.shapes import total_area

assert rectangle_area(2, 3) == 6
assert rectangle_area(2, 3, scale=2) == 12
assert total_area([(1, 1), (2, 2)]) == 5
print("ok")
'@
} }

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
$SmokeMissions = @(
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

$StressMissions = @(
    # --- 11 brownfield ---
    @{ name = 'bf-offbyone'; type = 'brownfield'
       prompt = 'Running `python test_slicer.py` fails: last_n in pkg/slicer.py is off by one and drops an element. Find and fix the bug so the test passes. Do not edit the test.'
       seed = { param($d) Seed-BfOffbyone $d }; verify = { param($d) Run-PyTest $d 'test_slicer.py' } },
    @{ name = 'bf-operator'; type = 'brownfield'
       prompt = 'Running `python test_grades.py` fails: is_passing in pkg/grades.py uses the wrong comparison — a score of exactly 60 should pass. Fix it so the test passes. Do not edit the test.'
       seed = { param($d) Seed-BfOperator $d }; verify = { param($d) Run-PyTest $d 'test_grades.py' } },
    @{ name = 'bf-mutable-default'; type = 'brownfield'
       prompt = 'Running `python test_collector.py` fails: add_item in pkg/collector.py uses a mutable default argument so state leaks between calls. Fix it so each call without a bag starts fresh. Do not edit the test.'
       seed = { param($d) Seed-BfMutableDefault $d }; verify = { param($d) Run-PyTest $d 'test_collector.py' } },
    @{ name = 'bf-regex'; type = 'brownfield'
       prompt = 'Running `node test.js` fails: isEmail in src/validate.js is too permissive (its regex lacks a domain dot and anchors). Tighten it so the assertions in test.js pass. Do not edit test.js.'
       seed = { param($d) Seed-BfRegex $d }; verify = { param($d) Run-NodeTest $d } },
    @{ name = 'bf-recursion'; type = 'brownfield'
       prompt = 'Running `python test_math2.py` fails: factorial in pkg/math2.py has a wrong base case (0! should be 1). Fix the base case so the test passes. Do not edit the test.'
       seed = { param($d) Seed-BfRecursion $d }; verify = { param($d) Run-PyTest $d 'test_math2.py' } },
    @{ name = 'bf-date'; type = 'brownfield'
       prompt = 'Running `python test_dates.py` fails: days_between in pkg/dates.py has its operands reversed and returns a negative count. Fix it so the test passes. Do not edit the test.'
       seed = { param($d) Seed-BfDate $d }; verify = { param($d) Run-PyTest $d 'test_dates.py' } },
    @{ name = 'bf-sort'; type = 'brownfield'
       prompt = 'Running `node test.js` fails: sortNums in src/sorting.js sorts lexicographically instead of numerically. Fix it with a numeric comparator so the assertions pass. Do not edit test.js.'
       seed = { param($d) Seed-BfSort $d }; verify = { param($d) Run-NodeTest $d } },
    @{ name = 'bf-null-guard'; type = 'brownfield'
       prompt = 'Running `node test.js` fails: getPort in src/config.js throws when cfg.server is missing. Add a guard so it returns the default port 8080 in that case. Do not edit test.js.'
       seed = { param($d) Seed-BfNullGuard $d }; verify = { param($d) Run-NodeTest $d } },
    @{ name = 'bf-accumulator'; type = 'brownfield'
       prompt = 'Running `python test_stats.py` fails: running_max in pkg/stats.py seeds the maximum at 0, which is wrong for all-negative input. Fix it so the test passes. Do not edit the test.'
       seed = { param($d) Seed-BfAccumulator $d }; verify = { param($d) Run-PyTest $d 'test_stats.py' } },
    @{ name = 'bf-boundary'; type = 'brownfield'
       prompt = 'Running `python test_clamp.py` fails: clamp in pkg/clampmod.py returns None for values already within range (it is missing a return). Fix it so in-range values are returned unchanged. Do not edit the test.'
       seed = { param($d) Seed-BfBoundary $d }; verify = { param($d) Run-PyTest $d 'test_clamp.py' } },
    @{ name = 'bf-string'; type = 'brownfield'
       prompt = 'Running `node test.js` fails: truncate in src/strings.js never shortens long strings. Make it return the string unchanged when its length is <= n; otherwise return the first (n-3) characters followed by "..." (three ASCII dots) so the result is exactly n characters. Do not edit test.js.'
       seed = { param($d) Seed-BfString $d }; verify = { param($d) Run-NodeTest $d } },

    # --- 3 multi-file refactor ---
    @{ name = 'rf-rename'; type = 'refactor'
       prompt = 'Running `python test_api.py` fails: it imports `compute`, but pkg/mathlib.py still defines the function as `calc` (used by pkg/report.py). Rename calc to compute everywhere it is defined and called so the test passes. Do not edit the test.'
       seed = { param($d) Seed-RfRename $d }; verify = { param($d) Run-PyTest $d 'test_api.py' } },
    @{ name = 'rf-extract'; type = 'refactor'
       prompt = 'Running `node test.js` fails: the `clamp` function is duplicated in src/a.js and src/b.js, and the test imports it from src/util.js. Extract clamp into a new src/util.js (export it), and have a.js and b.js import it from there instead of redefining it. Do not edit test.js.'
       seed = { param($d) Seed-RfExtract $d }; verify = { param($d) Run-NodeTest $d } },
    @{ name = 'rf-signature'; type = 'refactor'
       prompt = 'Running `python test_area.py` fails: rectangle_area in geo/area.py needs an optional `scale=1` parameter that multiplies the area, while existing callers (geo/shapes.py) must keep working unchanged. Update the signature accordingly so the test passes. Do not edit the test.'
       seed = { param($d) Seed-RfSignature $d }; verify = { param($d) Run-PyTest $d 'test_area.py' } },

    # --- 6 greenfield ---
    @{ name = 'gf-todo'; type = 'greenfield'
       prompt = 'Create a single-file to-do app in index.html (vanilla JS, no libraries): a text input and Add button to add tasks, each task with a checkbox to mark it done and a delete button, and the list must persist across page reloads using localStorage. Put all HTML, CSS, and JavaScript in index.html.'
       seed = { param($d) Seed-Greenfield $d 'gf-todo' }
       verify = { param($d) Verify-Files $d @('index.html') @('(?i)localstorage', 'addEventListener', '<input') } },
    @{ name = 'gf-pricing-table'; type = 'greenfield'
       prompt = 'Build a responsive pricing table in index.html + styles.css (no frameworks): three plan tiers (Basic, Pro, Enterprise), each with a price per month, a short feature list, and a sign-up button. Lay them out as three clean cards styled in styles.css.'
       seed = { param($d) Seed-Greenfield $d 'gf-pricing-table' }
       verify = { param($d) Verify-Files $d @('index.html', 'styles.css') @('(?i)pro|enterprise|basic', '\$\d', '(?i)month|/mo') } },
    @{ name = 'gf-markdown'; type = 'greenfield'
       prompt = 'Create a single-file Markdown previewer in index.html (vanilla JS): a textarea where the user types Markdown and a live-updating rendered HTML preview beside it. Support at least headings (#), bold (**text**), and links. Put everything in index.html.'
       seed = { param($d) Seed-Greenfield $d 'gf-markdown' }
       verify = { param($d) Verify-Files $d @('index.html') @('textarea', '(?i)innerhtml|insertadjacent|createelement|appendchild', 'replace') } },
    @{ name = 'gf-calculator'; type = 'greenfield'
       prompt = 'Build a working calculator web app in index.html (vanilla JS, no eval): number buttons 0-9, the + - * / operators, equals, and clear, with a display showing the current entry/result. Put all HTML, CSS, and JavaScript in index.html.'
       seed = { param($d) Seed-Greenfield $d 'gf-calculator' }
       verify = { param($d) Verify-Files $d @('index.html') @('<button', 'addEventListener', '(?i)display|result') } },
    @{ name = 'gf-snake'; type = 'greenfield'
       prompt = 'Create a playable Snake game in index.html using an HTML5 <canvas> and vanilla JavaScript: arrow keys steer the snake, it grows when it eats food, and the game ends on wall or self collision with the score shown. Put everything in index.html.'
       seed = { param($d) Seed-Greenfield $d 'gf-snake' }
       verify = { param($d) Verify-Files $d @('index.html') @('<canvas', '(?i)requestanimationframe|setinterval', '(?i)arrowup|arrowdown') } },
    @{ name = 'gf-saas-landing'; type = 'greenfield'
       prompt = "Build a modern SaaS landing page in index.html + styles.css for a fictional product called 'FlowState': a hero section with a headline and a call-to-action button, a three-card features section, a short pricing teaser, and a footer. Responsive, clean CSS in styles.css, no frameworks."
       seed = { param($d) Seed-Greenfield $d 'gf-saas-landing' }
       verify = { param($d) Verify-Files $d @('index.html', 'styles.css') @('(?i)flowstate', '(?i)feature', '(?i)footer') } }
)

$Missions = switch ($Set) {
    'smoke'  { $SmokeMissions }
    'stress' { $StressMissions }
    'all'    { $SmokeMissions + $StressMissions }
}

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

    $row = [pscustomobject]@{
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
    $results += $row
    # Append each result as it completes so a long/interrupted run is recoverable.
    if (-not $SeedOnly) {
        $row | Export-Csv -Path (Join-Path $WorkRoot "_results-$Set.csv") -Append -NoTypeInformation -Force
        Write-Host ("    -> {0}  {1}  loc={2} diff={3}  {4}s" -f $row.Verified, $row.Score, $row.Localized, $row.ApplyDiff, $row.Secs) -ForegroundColor DarkGray
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
