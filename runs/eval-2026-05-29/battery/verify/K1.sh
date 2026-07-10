#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1 TRANSCRIPT=$2 ; pass/fail/tc/tcre/tcount

# K1 (repo_map Java, locate) — the method `computeTax` is DEFINED only in
# TaxCalculator.java. Decoys: InvoiceService.java (defines computeTotal and only
# CALLS computeTax), PriceFormatter.java, Main.java. PASS iff the transcript
# names TaxCalculator.java; guard the pure wrong-answer case (only a decoy).
tc "TaxCalculator.java" || {
  if tc "InvoiceService.java" || tc "PriceFormatter.java" || tc "Main.java"; then
    fail "named a decoy file but not TaxCalculator.java (computeTax is defined there)"
  fi
  fail "transcript does not identify TaxCalculator.java as the definition site"
}
pass "located computeTax in TaxCalculator.java"
