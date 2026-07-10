#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1 TRANSCRIPT=$2 ; pass/fail/tc/tcre/tcount

# K5 (repo_map Kotlin, locate) — `data class Invoice` is defined only in
# Invoice.kt. Decoys: Order.kt (data class Order/Customer), Payment.kt (object
# PaymentGateway, class PaymentError). PASS iff the transcript names Invoice.kt.
tc "Invoice.kt" || {
  if tc "Order.kt" || tc "Payment.kt"; then
    fail "named a decoy file but not Invoice.kt (data class Invoice is defined there)"
  fi
  fail "transcript does not identify Invoice.kt as the definition site"
}
pass "located data class Invoice in Invoice.kt"
