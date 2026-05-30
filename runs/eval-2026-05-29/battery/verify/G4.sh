#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1  TRANSCRIPT=$2 ; fns pass/fail/tc/tcre/tcount

# G4 — rename CSS class btn-primary -> button-main across styles.css + index.html
# without changing styles. PASS iff: old name gone everywhere; new name appears
# as a rule in styles.css AND on >=2 elements in index.html; >=3 total; styles kept.
#
# rg-or-grep shim: the eval contract names `rg`, but verifiers run in a plain
# bash subprocess where rg may not be on PATH. Fall back to GNU grep.
cd "$WORKDIR" || fail "cannot cd into workdir"

[ -f "index.html" ] || fail "index.html missing"
[ -f "styles.css" ]  || fail "styles.css missing"

have_rg=0; command -v rg >/dev/null 2>&1 && have_rg=1

# any <ERE> [files...] : true if pattern matches anywhere
any() { local p="$1"; shift
  if [ "$have_rg" -eq 1 ]; then rg -q "$p" "$@"; else grep -rqE "$p" "$@"; fi
}
# count_occ <ERE> <file> : number of matching occurrences (one per line/instance)
count_occ() { local p="$1" f="$2"
  if [ "$have_rg" -eq 1 ]; then rg -o "$p" "$f" | wc -l | tr -d ' '
  else grep -oE "$p" "$f" | wc -l | tr -d ' '; fi
}

# Old class must be gone everywhere under the workdir.
if any 'btn-primary' .; then
  if [ "$have_rg" -eq 1 ]; then hits=$(rg -n 'btn-primary' .); else hits=$(grep -rnE 'btn-primary' .); fi
  fail "old class 'btn-primary' still present:\n$hits"
fi

# New rule must exist in styles.css (a .button-main selector).
any '\.button-main([^a-zA-Z0-9_-]|$)' styles.css || fail "no '.button-main' rule found in styles.css"

# New class must be used on >=2 elements in index.html.
html_uses=$(count_occ 'button-main' index.html)
[ "${html_uses:-0}" -ge 2 ] || fail "expected >=2 'button-main' usages in index.html, found ${html_uses:-0}"

# >=3 total occurrences of the new name across the two files (>=1 rule + >=2 usages).
css_uses=$(count_occ 'button-main' styles.css)
total=$(( ${html_uses:-0} + ${css_uses:-0} ))
[ "$total" -ge 3 ] || fail "expected >=3 total 'button-main' occurrences, found $total"

# Styles must be preserved: the declaration body should still carry its props.
any 'background:[[:space:]]*#2563eb' styles.css || fail "rule body changed (background color missing) — styles must be preserved"

pass "btn-primary -> button-main renamed in styles.css rule + ${html_uses} html usages (${total} total); styles intact"
