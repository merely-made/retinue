//! Minting on every core of a host.

use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use super::{Derivation, STAMP_LEN};

/// Attempts a thread makes between looks at whether another has found a stamp.
const BATCH: u64 = 1 << 10;

impl Derivation<'_> {
    /// [`Self::mint`] on `threads` threads sharing the finished midstate.
    ///
    /// Thread `i` walks from `seed` with its first byte XORed with `i`, so no two walks meet
    /// (one would need 2^248 steps to carry into that byte), and thread 0 walks
    /// [`Self::mint`]'s path. `attempts` is split between them. Which hit wins a race is not
    /// fixed, so the stamp can differ between runs from one seed. `None` before
    /// [`Self::done`].
    pub fn mint_parallel(
        &self,
        target: u16,
        mut seed: [u8; STAMP_LEN],
        attempts: u64,
        threads: usize,
    ) -> Option<([u8; STAMP_LEN], u16)> {
        // Each lane's mint would refuse at once, leaving it to spin out its whole share.
        if !self.done() || target > 256 {
            return None;
        }
        let threads = threads.clamp(1, 256);
        if threads == 1 {
            return self.mint(target, &mut seed, attempts);
        }
        let share = attempts.div_ceil(threads as u64);
        let found = AtomicBool::new(false);
        thread::scope(|scope| {
            let walks: Vec<_> = (0..threads)
                .map(|lane| {
                    let found = &found;
                    scope.spawn(move || {
                        let mut seed = seed;
                        seed[0] ^= lane as u8;
                        let mut left = share;
                        while left > 0 && !found.load(Ordering::Relaxed) {
                            let batch = left.min(BATCH);
                            if let Some(hit) = self.mint(target, &mut seed, batch) {
                                found.store(true, Ordering::Relaxed);
                                return Some(hit);
                            }
                            left -= batch;
                        }
                        None
                    })
                })
                .collect();
            walks
                .into_iter()
                .find_map(|walk| walk.join().expect("a mint thread does not panic"))
        })
    }
}

/// [`find_streamed`](super::find_streamed), minting on every core the host reports.
pub fn find_parallel(
    material: &[u8],
    rounds: u32,
    target: u16,
    seed: [u8; STAMP_LEN],
    max_attempts: u64,
) -> Option<([u8; STAMP_LEN], u16)> {
    if target > 256 {
        return None;
    }
    let mut derivation = Derivation::new(material, rounds);
    derivation.advance(rounds);
    let threads = thread::available_parallelism().map_or(1, NonZeroUsize::get);
    derivation.mint_parallel(target, seed, max_attempts, threads)
}

#[cfg(test)]
mod tests {
    use super::super::{find_streamed, value_streamed};
    use super::*;

    const MATERIAL: [u8; 32] = [0x2a; 32];

    fn derived(rounds: u32) -> Derivation<'static> {
        let mut derivation = Derivation::new(&MATERIAL, rounds);
        derivation.advance(rounds);
        derivation
    }

    #[test]
    fn one_thread_walks_the_sequential_path() {
        let sequential = find_streamed(&MATERIAL, 4, 10, [7; STAMP_LEN], 1 << 20);
        assert_eq!(
            derived(4).mint_parallel(10, [7; STAMP_LEN], 1 << 20, 1),
            sequential
        );
    }

    #[test]
    fn many_threads_find_a_stamp_that_scores() {
        let (stamp, value) = derived(4)
            .mint_parallel(12, [7; STAMP_LEN], 1 << 24, 8)
            .unwrap();
        assert!(value >= 12);
        assert_eq!(value_streamed(&MATERIAL, 4, &stamp), value);
    }

    #[test]
    fn an_exhausted_budget_or_an_impossible_target_finds_nothing() {
        assert_eq!(derived(1).mint_parallel(200, [0; STAMP_LEN], 64, 4), None);
        // Refused up front, not after spinning through an unbounded budget.
        assert_eq!(
            derived(1).mint_parallel(257, [0; STAMP_LEN], u64::MAX, 4),
            None
        );
        let unfinished = Derivation::new(&MATERIAL, 4);
        assert_eq!(
            unfinished.mint_parallel(1, [0; STAMP_LEN], u64::MAX, 4),
            None
        );
        assert_eq!(
            find_parallel(&MATERIAL, 1, 257, [0; STAMP_LEN], 1 << 20),
            None
        );
    }
}
