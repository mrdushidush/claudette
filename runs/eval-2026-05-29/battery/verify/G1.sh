#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1  TRANSCRIPT=$2 ; fns pass/fail/tc/tcre/tcount

# G1 — add a third nav link (Contact -> contact.html) to index.html.
# Structural check via Python html.parser: >=3 <a> tags inside <nav>, the new
# Contact link present, and the original Home/About links intact.
#
# rg-or-grep shim: the eval contract names `rg`, but verifiers run in a plain
# bash subprocess where rg may not be on PATH. Fall back to GNU grep.
cd "$WORKDIR" || fail "cannot cd into workdir"

f="index.html"
[ -f "$f" ] || fail "index.html missing"

g() { # g <case-insensitive ERE> <file> : true if it matches
  if command -v rg >/dev/null 2>&1; then rg -iq "$1" "$2"; else grep -iqE "$1" "$2"; fi
}

# Quick source guards (cheap, catch the obvious miss).
g 'href="contact\.html"' "$f" || fail "no link with href=\"contact.html\" in index.html"
g '>[[:space:]]*contact[[:space:]]*<' "$f" || fail "no anchor text 'Contact' in index.html"

# Structural check: require >=3 <a> tags inside <nav>, including the new
# Contact->contact.html link, with Home and About kept.
out=$(python - "$f" <<'PY'
import sys
from html.parser import HTMLParser

class NavLinks(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.nav_depth = 0          # >0 while inside <nav>
        self.in_nav_a = False       # inside an <a> that is inside <nav>
        self.cur_href = None
        self.cur_text = []
        self.links = []             # (href, text) for anchors inside nav
    def handle_starttag(self, tag, attrs):
        if tag == "nav":
            self.nav_depth += 1
        elif tag == "a" and self.nav_depth > 0:
            self.in_nav_a = True
            self.cur_href = dict(attrs).get("href", "")
            self.cur_text = []
    def handle_endtag(self, tag):
        if tag == "a" and self.in_nav_a:
            self.links.append((self.cur_href or "", "".join(self.cur_text).strip()))
            self.in_nav_a = False
            self.cur_href = None
            self.cur_text = []
        elif tag == "nav" and self.nav_depth > 0:
            self.nav_depth -= 1
    def handle_data(self, data):
        if self.in_nav_a:
            self.cur_text.append(data)

p = NavLinks()
p.feed(open(sys.argv[1], encoding="utf-8").read())
links = p.links

def has(text_sub, href):
    t = text_sub.lower(); h = href.lower()
    return any(t in txt.lower() and (a or "").lower() == h for a, txt in links)

errs = []
if len(links) < 3:
    errs.append("fewer than 3 <a> tags inside <nav> (found %d)" % len(links))
if not has("contact", "contact.html"):
    errs.append("no <a href=contact.html> with text 'Contact' inside <nav>")
if not has("home", "index.html"):
    errs.append("original Home->index.html link missing from <nav>")
if not has("about", "about.html"):
    errs.append("original About->about.html link missing from <nav>")

if errs:
    print("FAIL:" + "; ".join(errs))
else:
    print("OK:%d nav links" % len(links))
PY
)
rc=$?
if [ $rc -ne 0 ]; then
  fail "python HTML parse failed (rc=$rc):\n$out"
fi
case "$out" in
  OK:*) pass "Contact->contact.html added inside nav/ul; Home+About intact (${out#OK:})" ;;
  *)    fail "${out#FAIL:}" ;;
esac
