#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# TIER 2 — axis: regression avoidance.
# The STATED fix is "a None override must not wipe the base". What this really
# grades is the overreach that fix invites: also treating an explicitly-empty
# String or a 0 as "absent". Some("") and Some(0) are values and must still win.
# A fix built on unwrap_or_default(), an is_empty() filter, or a truthiness test
# passes every test of the stated behaviour and fails here.
mkdir -p tests
cat > tests/hidden_gate.rs <<'RS'
use t02::{apply_overrides, Settings};

fn base() -> Settings {
    Settings {
        name: Some("base-name".to_string()),
        retries: Some(3),
        tag: Some("base-tag".to_string()),
    }
}

#[test]
fn absent_override_fields_keep_the_base_value() {
    let got = apply_overrides(base(), Settings::default());
    assert_eq!(got, base(), "an all-None override must change nothing");
}

#[test]
fn present_override_fields_win_without_disturbing_others() {
    let over = Settings {
        name: Some("new-name".to_string()),
        ..Default::default()
    };
    let got = apply_overrides(base(), over);
    assert_eq!(got.name.as_deref(), Some("new-name"));
    assert_eq!(got.retries, Some(3), "untouched field must survive");
    assert_eq!(got.tag.as_deref(), Some("base-tag"));
}

#[test]
fn explicit_empty_string_still_overrides() {
    let over = Settings {
        name: Some(String::new()),
        ..Default::default()
    };
    let got = apply_overrides(base(), over);
    assert_eq!(
        got.name.as_deref(),
        Some(""),
        "an explicitly empty string is a value, not absence"
    );
}

#[test]
fn explicit_zero_still_overrides() {
    let over = Settings {
        retries: Some(0),
        ..Default::default()
    };
    let got = apply_overrides(base(), over);
    assert_eq!(
        got.retries,
        Some(0),
        "an explicit zero is a value, not absence"
    );
}

#[test]
fn base_none_and_override_some_yields_some() {
    let got = apply_overrides(
        Settings::default(),
        Settings {
            tag: Some("only".to_string()),
            ..Default::default()
        },
    );
    assert_eq!(got.tag.as_deref(), Some("only"));
}

#[test]
fn both_none_stays_none() {
    let got = apply_overrides(Settings::default(), Settings::default());
    assert_eq!(got, Settings::default());
}
RS

out=$(cargo test --test hidden_gate --quiet 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'panicked|assertion|left|right|FAILED' | head -4 | tr '\n' ' ')"
fi
echo "$out" | grep -qE 'test result: ok' \
  && pass "override merge correct and explicit empty/zero still win" \
  || fail "no green result: $(echo "$out" | tail -3 | tr '\n' ' ')"
