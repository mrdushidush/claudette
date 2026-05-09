//! Forge 7-stage pipeline scaffolding.
//!
//! `Router -> Planner -> Coder -> TestCoder -> Verifier -> SurgicalCoder -> Gate`
//!
//! Verifier implicitly loops back to SurgicalCoder until the Gate threshold is
//! met or the max-rounds budget is exhausted. Fix policy: surgical by default;
//! full regen only at round 1 when score < 8.5 AND compile failed.
//!
//! Double-Context Phase-0 gambit: the Coder stage may try 2x context on its
//! first attempt; if the Verifier passes, the retry ladder is skipped.
//!
//! Ported from `claudettes-forge/crates/forge/src/lib.rs` at the `rc1-final`
//! tag. Every module is a `pub mod` placeholder — none are wired into
//! claudette in 0.4.1.

pub mod router {
    //! Campbell Complexity 1-10 + provider selection.
}

pub mod planner {
    //! Mission decomposition into subtasks.
}

pub mod coder {
    //! Code generation. Double-Context Phase-0 gambit lives here.
}

pub mod test_coder {
    //! Test generation.
}

pub mod verifier_stage {
    //! In-pipeline Verifier stage (delegates to a future verifier crate).
}

pub mod surgical_coder {
    //! Fix-pass. Surgical by default; regen fallback only round 1 under
    //! specific conditions.
}

pub mod gate {
    //! Final pass/fail decision + score.
}
