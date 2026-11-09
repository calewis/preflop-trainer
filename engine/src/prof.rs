//! Phase timers, compiled in only with the `prof` feature. Each worker thread
//! keeps its own cycle totals and hands them over with `take`.

pub const SIM: usize = 0;
pub const BUCKETS: usize = 1;
pub const POOL: usize = 2;
pub const SOLVE_TOTAL: usize = 3;
pub const FLOP_VEC: usize = 4;
pub const DECORR: usize = 5;
pub const BUILD_TURN: usize = 6;
pub const FILL_RIVER: usize = 7;
pub const SHOWDOWN: usize = 8;
pub const TAPE: usize = 9;
pub const NPHASE: usize = 10;

pub const NAMES: [&str; NPHASE] = [
    "simulation", "bucketing", "root_posterior", "solve_total", "flop_vectors", "decorrelate", "build_turn", "fill_river", "showdown", "tape_compile",
];

#[cfg(feature = "prof")]
mod imp {
    use super::NPHASE;
    use std::cell::Cell;
    thread_local! {
        static TOTALS: Cell<[u64; NPHASE]> = const { Cell::new([0; NPHASE]) };
    }
    #[inline(always)]
    pub fn span<R>(phase: usize, f: impl FnOnce() -> R) -> R {
        // SAFETY: rdtsc has no preconditions on x86_64.
        let t0 = unsafe { core::arch::x86_64::_rdtsc() };
        let r = f();
        let t1 = unsafe { core::arch::x86_64::_rdtsc() };
        TOTALS.with(|c| {
            let mut a = c.get();
            a[phase] += t1 - t0;
            c.set(a);
        });
        r
    }
    pub fn take() -> [u64; NPHASE] {
        TOTALS.with(|c| c.replace([0; NPHASE]))
    }
}

#[cfg(not(feature = "prof"))]
mod imp {
    use super::NPHASE;
    #[inline(always)]
    pub fn span<R>(_phase: usize, f: impl FnOnce() -> R) -> R {
        f()
    }
    pub fn take() -> [u64; NPHASE] {
        [0; NPHASE]
    }
}

pub use imp::{span, take};
