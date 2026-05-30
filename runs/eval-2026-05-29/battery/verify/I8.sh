#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
# GT: compact when estimated tokens >= 10_000 AND >4 msgs; summarize older into a system summary, keep recent.
thresh=0; mech=0
tcre '10[,_ ]?000|10k|max_estimated_tokens|token.{0,15}(budget|threshold|limit)|exceed.{0,15}token|estimate.{0,15}token|too (many|large|big)' && thresh=1
tcre 'summar|condens|preserve|recent|older message|earlier message|drop.{0,10}old' && mech=1
{ [ "$thresh" -eq 1 ] && [ "$mech" -eq 1 ]; } && pass "described threshold+summarize mechanism" || fail "missing threshold($thresh) or mechanism($mech)"
