#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1 TRANSCRIPT=$2 ; pass/fail/tc/tcre/tcount

# K3 (repo_map C#, locate) — ValidateOrder is DEFINED only in OrderValidator.cs.
# Decoys: CustomerValidator.cs (ValidateCustomer), OrderRepository.cs (Save/Load),
# Models.cs (data types). PASS iff the transcript names OrderValidator.cs.
tc "OrderValidator.cs" || {
  if tc "CustomerValidator.cs" || tc "OrderRepository.cs" || tc "Models.cs"; then
    fail "named a decoy file but not OrderValidator.cs (ValidateOrder is defined there)"
  fi
  fail "transcript does not identify OrderValidator.cs as the definition site"
}
pass "located ValidateOrder in OrderValidator.cs"
