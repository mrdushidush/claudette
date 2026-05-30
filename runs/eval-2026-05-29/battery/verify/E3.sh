#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1 TRANSCRIPT=$2 ; pass/fail/tc/tcre/tcount

# E3 — locate symbol (TRANSCRIPT check). `func handleCheckout(` is defined ONLY in
# handlers/checkout.go. Decoys: handleLogin (handlers/auth.go), handleHome
# (handlers/home.go), and a "/checkout" route STRING in router/router.go and
# handlers/routes.go. The agent must name checkout.go as the defining file.

# Must point at checkout.go.
tc "checkout.go" || fail "transcript does not name checkout.go as the defining file"

# Must NOT misattribute to a decoy handler file as the answer.
# (We only fail if a decoy file is named WITHOUT also naming checkout.go — but
# checkout.go is already required above, so a transcript that mentions both the
# correct file and, say, auth.go in passing still passes. Guard only the pure
# wrong-answer case: names a decoy but never checkout.go is already caught.)

pass "transcript identifies handlers/checkout.go as defining handleCheckout"
