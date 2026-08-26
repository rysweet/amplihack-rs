//! Machine-checked proofs of the spawn-accounting decisions (issue #1329).
//!
//! These are not tests. A test tries the inputs someone thought of; Kani asks a
//! solver whether ANY input can break the claim, and either proves none can or hands
//! back the one that does.
//!
//! Scope: the pure decision functions. Everything they decide -- how deep a tree may
//! go, where its state lives, how wide a launch may be -- is settled here for every
//! possible input, so those answers cannot be wrong. What happens *around* them
//! (spawning, locking, crashing) is concurrency and belongs to `docs/spec/OrchLedger.tla`.
//!
//! Run with: `cargo kani -p amplihack-cli`

#[cfg(kani)]
mod proofs {
    use crate::commands::session_tree::state::{
        DEFAULT_MAX_DEPTH, MAX_DEPTH_CEILING, effective_max_depth,
    };

    /// CeilingMonotone, for every input rather than the handful a test tries.
    ///
    /// The environment may lower a sealed ceiling and can never raise it. This is the
    /// property the escalation ladder (5 -> 6 -> 7 -> 8 -> 9) attacked.
    #[kani::proof]
    fn effective_max_depth_never_exceeds_the_seal() {
        let sealed: u32 = kani::any();
        let env: Option<u32> = kani::any();
        kani::assume(sealed <= MAX_DEPTH_CEILING);

        let got = effective_max_depth(Some(sealed), env);
        assert!(got <= sealed);
    }

    /// No input escapes the hard ceiling, sealed or not.
    #[kani::proof]
    fn effective_max_depth_always_respects_the_hard_ceiling() {
        let sealed: Option<u32> = kani::any();
        let env: Option<u32> = kani::any();
        assert!(effective_max_depth(sealed, env) <= MAX_DEPTH_CEILING);
    }

    /// Lowering must actually work, or the safety property could be satisfied by a
    /// function that always returns 0 and nesting would be dead.
    #[kani::proof]
    fn effective_max_depth_honours_a_lower_request() {
        let sealed: u32 = kani::any();
        let env: u32 = kani::any();
        kani::assume(sealed <= MAX_DEPTH_CEILING);
        kani::assume(env <= sealed);

        assert_eq!(effective_max_depth(Some(sealed), Some(env)), env);
    }

    /// An unsealed tree falls back to the environment, still clamped -- otherwise a
    /// root could never establish a ceiling.
    #[kani::proof]
    fn unsealed_falls_back_to_the_environment() {
        let env: Option<u32> = kani::any();
        let got = effective_max_depth(None, env);
        match env {
            Some(v) => assert_eq!(
                got,
                if v > MAX_DEPTH_CEILING {
                    MAX_DEPTH_CEILING
                } else {
                    v
                }
            ),
            None => assert_eq!(got, DEFAULT_MAX_DEPTH),
        }
    }

    /// Fan-out: the launch limit is never zero unless explicitly asked for, and a
    /// wave never exceeds it. A limit of 0 that was not requested would launch
    /// nothing; a wave larger than the limit defeats the point.
    #[kani::proof]
    fn launch_waves_respect_their_limit() {
        use crate::commands::multitask::waves::{limit_from, wave_size};
        let cpus: usize = kani::any();
        kani::assume(cpus >= 1 && cpus <= 1024);

        let limit = limit_from(None, cpus);
        assert!(limit >= 1, "an unconfigured limit must let work proceed");
        assert!(limit <= 8, "an unconfigured limit must stay capped");

        let count: usize = kani::any();
        kani::assume(count <= 1024);
        let wave = wave_size(count, limit);
        assert!(wave <= limit);
        assert!(wave >= 1 || count == 0);
    }

    /// A depth claim is only acted on when something corroborates it. Uncorroborated,
    /// it must be discarded -- this is what stops a stale environment variable in a
    /// surviving shell from wedging a user permanently.
    #[kani::proof]
    fn an_uncorroborated_depth_claim_is_discarded() {
        use crate::commands::recipe::run::execute::resolve_claimed_depth;
        let claimed: u32 = kani::any();
        let sealed: Option<u32> = kani::any();
        let corroborated: bool = kani::any();

        let got = resolve_claimed_depth(claimed, sealed, corroborated);

        if claimed > 0 && sealed.is_none() && !corroborated {
            assert_eq!(got, 0, "nothing vouches for it, so it must be discarded");
        } else {
            assert_eq!(got, claimed, "a corroborated or sealed claim must stand");
        }
    }

    /// The function is total: no input panics, overflows, or diverges.
    #[kani::proof]
    fn effective_max_depth_is_total() {
        let sealed: Option<u32> = kani::any();
        let env: Option<u32> = kani::any();
        let _ = effective_max_depth(sealed, env);
    }
}
