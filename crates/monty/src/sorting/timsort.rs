//! A port of CPython's `listsort` (`Objects/listobject.c`, 3.14): timsort with the powersort merge policy.
//!
//! Python comparisons need not be a total order (`NaN`, sets, comparisons that raise), and the order CPython leaves
//! such input in is observable. Porting the algorithm step for step, asking only `lt(a, b)` in the same sequence,
//! reproduces that order exactly, which no Rust std sort can do (and `slice::sort_by` panics on an inconsistent
//! comparator).
//!
//! The sort permutes `usize` element ids rather than the values, so merges can copy freely without `unsafe` or
//! refcount traffic; [`sort_short`] moves the values themselves when no merge is needed. A failed comparison stops
//! the sort at once; like CPython, the ids are still a permutation of the input when the error is returned, so
//! callers may apply them either way.

use std::{cmp::min, mem};

/// When a merge enters galloping mode it stays there until both runs win fewer than this many times in a row.
const MIN_GALLOP: usize = 7;

/// Upper bound of the minimum run length; a power of two, as `merge_compute_minrun` requires.
const MAX_MINRUN: usize = 64;

/// Returned by the comparison to stop the sort; the caller keeps the actual error.
///
/// Zero-sized so a comparison's result fits in a register: comparisons dominate the sort's cost.
#[derive(Clone, Copy)]
pub(super) struct SortAborted;

/// Sorts `ids` stably, ordering by `lt`, which is called with two element ids and answers "is the first less than
/// the second". The first [`SortAborted`] from `lt` aborts the sort; `ids` is then some permutation of its input.
pub(super) fn timsort(
    ids: &mut [usize],
    lt: impl FnMut(usize, usize) -> Result<bool, SortAborted>,
) -> Result<(), SortAborted> {
    let list_len = ids.len();
    if list_len < 2 {
        return Ok(());
    }
    let mut state = MergeState {
        lt,
        min_gallop: MIN_GALLOP,
        tmp: Vec::new(),
        pending: Vec::new(),
        list_len,
    };
    let minrun = merge_compute_minrun(list_len);
    let mut lo = 0;
    while lo < list_len {
        let remaining = list_len - lo;
        let lt = &mut |x: &usize, y: &usize| (state.lt)(*x, *y);
        let mut n = count_run(&mut ids[lo..], lt)?;
        // Extend a short natural run to `minrun` with binary insertion.
        if n < minrun {
            let force = min(remaining, minrun);
            binary_sort(&mut ids[lo..lo + force], n, lt)?;
            n = force;
        }
        state.found_new_run(ids, n)?;
        state.pending.push(Run {
            base: lo,
            len: n,
            power: 0,
        });
        lo += n;
    }
    state.merge_force_collapse(ids)
}

/// Sorts a slice shorter than 64 elements in place, exactly as [`timsort`] would sort ids for it.
///
/// Below 64 elements CPython's sort is one natural run extended by binary insertion, with no merge, so it needs no
/// scratch copies and can move the values themselves; that spares short sorts the id buffer.
pub(super) fn sort_short<T>(
    a: &mut [T],
    mut lt: impl FnMut(&T, &T) -> Result<bool, SortAborted>,
) -> Result<(), SortAborted> {
    debug_assert!(a.len() < MAX_MINRUN);
    if a.len() < 2 {
        return Ok(());
    }
    let n = count_run(a, &mut lt)?;
    if n < a.len() {
        binary_sort(a, n, &mut lt)?;
    }
    Ok(())
}

/// Stable binary insertion sort of `a`, whose first `ok` elements are already sorted.
fn binary_sort<T>(
    a: &mut [T],
    ok: usize,
    lt: &mut impl FnMut(&T, &T) -> Result<bool, SortAborted>,
) -> Result<(), SortAborted> {
    for ok in ok.max(1)..a.len() {
        let (mut l, mut r) = (0, ok);
        while l < r {
            let m = usize::midpoint(l, r);
            if lt(&a[ok], &a[m])? {
                r = m;
            } else {
                l = m + 1;
            }
        }
        a[l..=ok].rotate_right(1);
    }
    Ok(())
}

/// Returns the length of the run at the start of `a`, reversing it in place first if it is descending.
///
/// A descending run may contain equal neighbours; each such group is reversed on the fly so the final
/// whole-run reversal restores their original order, keeping the sort stable.
fn count_run<T>(a: &mut [T], lt: &mut impl FnMut(&T, &T) -> Result<bool, SortAborted>) -> Result<usize, SortAborted> {
    let remaining = a.len();
    let mut n = 1;
    // Try an ascending run first.
    while n < remaining && !lt(&a[n], &a[n - 1])? {
        n += 1;
    }
    if n == remaining {
        return Ok(n);
    }
    // `a[n]` is strictly less. If the ascending prefix rose anywhere, it is the run; otherwise its elements are
    // all equal, and it starts a descending run.
    if n > 1 {
        if lt(&a[0], &a[n - 1])? {
            return Ok(n);
        }
        a[..n].reverse();
    }
    n += 1;

    // Finish the descending run; `neq` counts the trailing equal neighbours seen so far.
    let mut neq = 0;
    while n < remaining {
        if lt(&a[n], &a[n - 1])? {
            reverse_last_equal(a, n, &mut neq);
        } else if lt(&a[n - 1], &a[n])? {
            break;
        } else {
            neq += 1;
        }
        n += 1;
    }
    reverse_last_equal(a, n, &mut neq);
    a[..n].reverse();

    // The reversed run may continue with an ascending suffix, e.g. `[3, 2, 3, 4, 1]`.
    while n < remaining && !lt(&a[n], &a[n - 1])? {
        n += 1;
    }
    Ok(n)
}

/// A sorted run awaiting a merge: `ids[base..base + len]`.
#[derive(Clone, Copy)]
struct Run {
    base: usize,
    len: usize,
    /// The powersort node level separating this run from the next one.
    power: u32,
}

/// What `merge_lo` / `merge_hi` do once their main loop stops.
enum MergeEnd {
    /// Copy the remaining scratch elements back into place.
    Drain,
    /// One element of the scratch run is left; it goes after (or before) the rest of the other run.
    CopyLast,
}

/// State shared by one sort's helpers, mirroring CPython's `MergeState`.
struct MergeState<F> {
    lt: F,
    /// Adaptive galloping threshold, nudged by each merge.
    min_gallop: usize,
    /// Scratch space holding the smaller run while it is merged.
    tmp: Vec<usize>,
    /// Stack of runs yet to be merged; adjacent entries are adjacent in `ids`.
    pending: Vec<Run>,
    list_len: usize,
}

impl<F: FnMut(usize, usize) -> Result<bool, SortAborted>> MergeState<F> {
    /// Records a new run of length `n2` after the top of the stack, first merging any pending runs whose powersort
    /// level is deeper than the boundary the new run creates.
    fn found_new_run(&mut self, ids: &mut [usize], n2: usize) -> Result<(), SortAborted> {
        let Some(last) = self.pending.last() else {
            return Ok(());
        };
        let power = powerloop(last.base, last.len, n2, self.list_len);
        while self.pending.len() > 1 && self.pending[self.pending.len() - 2].power > power {
            self.merge_at(ids, self.pending.len() - 2)?;
        }
        let top = self.pending.len() - 1;
        self.pending[top].power = power;
        Ok(())
    }

    /// Merges every pending run, leaving one.
    fn merge_force_collapse(&mut self, ids: &mut [usize]) -> Result<(), SortAborted> {
        while self.pending.len() > 1 {
            let mut n = self.pending.len() - 2;
            if n > 0 && self.pending[n - 1].len < self.pending[n + 1].len {
                n -= 1;
            }
            self.merge_at(ids, n)?;
        }
        Ok(())
    }

    /// Merges the pending runs at stack indices `i` and `i + 1`.
    fn merge_at(&mut self, ids: &mut [usize], i: usize) -> Result<(), SortAborted> {
        let Run {
            base: mut base_a,
            len: mut na,
            ..
        } = self.pending[i];
        let Run {
            base: base_b, len: nb, ..
        } = self.pending[i + 1];
        self.pending[i].len = na + nb;
        if i + 3 == self.pending.len() {
            self.pending[i + 1] = self.pending[i + 2];
        }
        self.pending.pop();

        // Elements of A before where B's first element belongs are already in place.
        let k = self.gallop_right(ids[base_b], &ids[base_a..base_a + na], 0)?;
        base_a += k;
        na -= k;
        if na == 0 {
            return Ok(());
        }
        // Likewise elements of B after where A's last element belongs.
        let nb = self.gallop_left(ids[base_a + na - 1], &ids[base_b..base_b + nb], nb - 1)?;
        if nb == 0 {
            return Ok(());
        }
        if na <= nb {
            self.merge_lo(ids, base_a, na, nb)
        } else {
            self.merge_hi(ids, base_a, na, nb)
        }
    }

    /// Returns `k` such that `a[k - 1] < key <= a[k]`: where `key` goes, left of any equal elements.
    ///
    /// The search gallops outwards from `hint` before binary searching, so a good hint makes it cheap.
    fn gallop_left(&mut self, key: usize, a: &[usize], hint: usize) -> Result<usize, SortAborted> {
        let n = a.len();
        let mut last_ofs = 0;
        let mut ofs = 1;
        let (mut lo, hi) = if (self.lt)(a[hint], key)? {
            // a[hint] < key: gallop right until a[hint + last_ofs] < key <= a[hint + ofs].
            let max_ofs = n - hint;
            while ofs < max_ofs {
                if (self.lt)(a[hint + ofs], key)? {
                    last_ofs = ofs;
                    ofs = (ofs << 1) + 1;
                } else {
                    break;
                }
            }
            // `lo` is one past an index known to be < key.
            (hint + last_ofs + 1, hint + min(ofs, max_ofs))
        } else {
            // key <= a[hint]: gallop left until a[hint - ofs] < key <= a[hint - last_ofs].
            let max_ofs = hint + 1;
            while ofs < max_ofs {
                if (self.lt)(a[hint - ofs], key)? {
                    break;
                }
                last_ofs = ofs;
                ofs = (ofs << 1) + 1;
            }
            // CPython's `lastofs` may be -1 here; this is it plus one.
            (hint + 1 - min(ofs, max_ofs), hint - last_ofs)
        };
        let mut hi = hi;
        while lo < hi {
            let m = lo + ((hi - lo) >> 1);
            if (self.lt)(a[m], key)? {
                lo = m + 1;
            } else {
                hi = m;
            }
        }
        Ok(hi)
    }

    /// Returns `k` such that `a[k - 1] <= key < a[k]`: where `key` goes, right of any equal elements.
    fn gallop_right(&mut self, key: usize, a: &[usize], hint: usize) -> Result<usize, SortAborted> {
        let n = a.len();
        let mut last_ofs = 0;
        let mut ofs = 1;
        let (mut lo, hi) = if (self.lt)(key, a[hint])? {
            // key < a[hint]: gallop left until a[hint - ofs] <= key < a[hint - last_ofs].
            let max_ofs = hint + 1;
            while ofs < max_ofs {
                if (self.lt)(key, a[hint - ofs])? {
                    last_ofs = ofs;
                    ofs = (ofs << 1) + 1;
                } else {
                    break;
                }
            }
            (hint + 1 - min(ofs, max_ofs), hint - last_ofs)
        } else {
            // a[hint] <= key: gallop right until a[hint + last_ofs] <= key < a[hint + ofs].
            let max_ofs = n - hint;
            while ofs < max_ofs {
                if (self.lt)(key, a[hint + ofs])? {
                    break;
                }
                last_ofs = ofs;
                ofs = (ofs << 1) + 1;
            }
            (hint + last_ofs + 1, hint + min(ofs, max_ofs))
        };
        let mut hi = hi;
        while lo < hi {
            let m = lo + ((hi - lo) >> 1);
            if (self.lt)(key, a[m])? {
                hi = m;
            } else {
                lo = m + 1;
            }
        }
        Ok(hi)
    }

    /// Merges run A (`ids[base_a..base_a + na]`) with the run B that follows it, copying A to scratch and filling
    /// from the left. Used when `na <= nb`; requires A's first element to belong first and B's last to belong last.
    fn merge_lo(&mut self, ids: &mut [usize], base_a: usize, na: usize, nb: usize) -> Result<(), SortAborted> {
        self.tmp.clear();
        self.tmp.extend_from_slice(&ids[base_a..base_a + na]);
        let mut cur = LoCursor {
            dest: base_a,
            pa: 0,
            pb: base_a + na,
            na,
            nb,
        };
        let result = self.merge_lo_loop(ids, &mut cur);
        let LoCursor { dest, pa, pb, na, nb } = cur;
        match result {
            Ok(MergeEnd::CopyLast) => {
                // The last element of A belongs after everything left in B.
                ids.copy_within(pb..pb + nb, dest);
                ids[dest + nb] = self.tmp[pa];
                Ok(())
            }
            // Success or failure, whatever is left of A goes back so `ids` stays a permutation.
            other => {
                ids[dest..dest + na].copy_from_slice(&self.tmp[pa..pa + na]);
                other.map(drop)
            }
        }
    }

    /// The body of [`merge_lo`](Self::merge_lo), split out so `?` can stop it while the caller tidies up.
    fn merge_lo_loop(&mut self, ids: &mut [usize], c: &mut LoCursor) -> Result<MergeEnd, SortAborted> {
        ids[c.dest] = ids[c.pb];
        c.dest += 1;
        c.pb += 1;
        c.nb -= 1;
        if c.nb == 0 {
            return Ok(MergeEnd::Drain);
        }
        if c.na == 1 {
            return Ok(MergeEnd::CopyLast);
        }
        let mut min_gallop = self.min_gallop;
        loop {
            // Straightforward merge until one run wins `min_gallop` times in a row.
            let mut acount = 0;
            let mut bcount = 0;
            loop {
                if (self.lt)(ids[c.pb], self.tmp[c.pa])? {
                    ids[c.dest] = ids[c.pb];
                    c.dest += 1;
                    c.pb += 1;
                    bcount += 1;
                    acount = 0;
                    c.nb -= 1;
                    if c.nb == 0 {
                        return Ok(MergeEnd::Drain);
                    }
                    if bcount >= min_gallop {
                        break;
                    }
                } else {
                    ids[c.dest] = self.tmp[c.pa];
                    c.dest += 1;
                    c.pa += 1;
                    acount += 1;
                    bcount = 0;
                    c.na -= 1;
                    if c.na == 1 {
                        return Ok(MergeEnd::CopyLast);
                    }
                    if acount >= min_gallop {
                        break;
                    }
                }
            }

            // Gallop until neither run is winning consistently.
            min_gallop += 1;
            loop {
                min_gallop -= usize::from(min_gallop > 1);
                self.min_gallop = min_gallop;
                let k = self.gallop_right_tmp(ids[c.pb], c.pa, c.na)?;
                acount = k;
                if k > 0 {
                    ids[c.dest..c.dest + k].copy_from_slice(&self.tmp[c.pa..c.pa + k]);
                    c.dest += k;
                    c.pa += k;
                    c.na -= k;
                    if c.na == 1 {
                        return Ok(MergeEnd::CopyLast);
                    }
                    // Only reachable with an inconsistent comparison.
                    if c.na == 0 {
                        return Ok(MergeEnd::Drain);
                    }
                }
                ids[c.dest] = ids[c.pb];
                c.dest += 1;
                c.pb += 1;
                c.nb -= 1;
                if c.nb == 0 {
                    return Ok(MergeEnd::Drain);
                }

                let k = self.gallop_left(self.tmp[c.pa], &ids[c.pb..c.pb + c.nb], 0)?;
                bcount = k;
                if k > 0 {
                    ids.copy_within(c.pb..c.pb + k, c.dest);
                    c.dest += k;
                    c.pb += k;
                    c.nb -= k;
                    if c.nb == 0 {
                        return Ok(MergeEnd::Drain);
                    }
                }
                ids[c.dest] = self.tmp[c.pa];
                c.dest += 1;
                c.pa += 1;
                c.na -= 1;
                if c.na == 1 {
                    return Ok(MergeEnd::CopyLast);
                }
                if acount < MIN_GALLOP && bcount < MIN_GALLOP {
                    break;
                }
            }
            // Penalise leaving galloping mode.
            min_gallop += 1;
            self.min_gallop = min_gallop;
        }
    }

    /// Merges run A (`ids[base_a..base_a + na]`) with the run B of length `nb` that follows it, copying B to scratch
    /// and filling from the right. Used when `na > nb`; same preconditions as [`merge_lo`](Self::merge_lo).
    fn merge_hi(&mut self, ids: &mut [usize], base_a: usize, na: usize, nb: usize) -> Result<(), SortAborted> {
        let base_b = base_a + na;
        self.tmp.clear();
        self.tmp.extend_from_slice(&ids[base_b..base_b + nb]);
        // Cursors point one past the next element to move, so they never go below zero.
        let mut cur = HiCursor {
            dest: base_b + nb,
            pa: base_b,
            pb: nb,
            na,
            nb,
        };
        let result = self.merge_hi_loop(ids, base_a, &mut cur);
        let HiCursor { dest, pa, nb, na, .. } = cur;
        match result {
            Ok(MergeEnd::CopyLast) => {
                // The first element of B belongs before everything left in A.
                ids.copy_within(pa - na..pa, dest - na);
                ids[dest - na - 1] = self.tmp[0];
                Ok(())
            }
            other => {
                ids[dest - nb..dest].copy_from_slice(&self.tmp[..nb]);
                other.map(drop)
            }
        }
    }

    /// The body of [`merge_hi`](Self::merge_hi), split out so `?` can stop it while the caller tidies up.
    fn merge_hi_loop(&mut self, ids: &mut [usize], base_a: usize, c: &mut HiCursor) -> Result<MergeEnd, SortAborted> {
        c.dest -= 1;
        c.pa -= 1;
        ids[c.dest] = ids[c.pa];
        c.na -= 1;
        if c.na == 0 {
            return Ok(MergeEnd::Drain);
        }
        if c.nb == 1 {
            return Ok(MergeEnd::CopyLast);
        }
        let mut min_gallop = self.min_gallop;
        loop {
            let mut acount = 0;
            let mut bcount = 0;
            loop {
                if (self.lt)(self.tmp[c.pb - 1], ids[c.pa - 1])? {
                    c.dest -= 1;
                    c.pa -= 1;
                    ids[c.dest] = ids[c.pa];
                    acount += 1;
                    bcount = 0;
                    c.na -= 1;
                    if c.na == 0 {
                        return Ok(MergeEnd::Drain);
                    }
                    if acount >= min_gallop {
                        break;
                    }
                } else {
                    c.dest -= 1;
                    c.pb -= 1;
                    ids[c.dest] = self.tmp[c.pb];
                    bcount += 1;
                    acount = 0;
                    c.nb -= 1;
                    if c.nb == 1 {
                        return Ok(MergeEnd::CopyLast);
                    }
                    if bcount >= min_gallop {
                        break;
                    }
                }
            }

            min_gallop += 1;
            loop {
                min_gallop -= usize::from(min_gallop > 1);
                self.min_gallop = min_gallop;
                let k = c.na - self.gallop_right(self.tmp[c.pb - 1], &ids[base_a..base_a + c.na], c.na - 1)?;
                acount = k;
                if k > 0 {
                    c.dest -= k;
                    c.pa -= k;
                    ids.copy_within(c.pa..c.pa + k, c.dest);
                    c.na -= k;
                    if c.na == 0 {
                        return Ok(MergeEnd::Drain);
                    }
                }
                c.dest -= 1;
                c.pb -= 1;
                ids[c.dest] = self.tmp[c.pb];
                c.nb -= 1;
                if c.nb == 1 {
                    return Ok(MergeEnd::CopyLast);
                }

                let k = c.nb - self.gallop_left_tmp(ids[c.pa - 1], c.nb)?;
                bcount = k;
                if k > 0 {
                    c.dest -= k;
                    c.pb -= k;
                    ids[c.dest..c.dest + k].copy_from_slice(&self.tmp[c.pb..c.pb + k]);
                    c.nb -= k;
                    if c.nb == 1 {
                        return Ok(MergeEnd::CopyLast);
                    }
                    // Only reachable with an inconsistent comparison.
                    if c.nb == 0 {
                        return Ok(MergeEnd::Drain);
                    }
                }
                c.dest -= 1;
                c.pa -= 1;
                ids[c.dest] = ids[c.pa];
                c.na -= 1;
                if c.na == 0 {
                    return Ok(MergeEnd::Drain);
                }
                if acount < MIN_GALLOP && bcount < MIN_GALLOP {
                    break;
                }
            }
            min_gallop += 1;
            self.min_gallop = min_gallop;
        }
    }

    /// [`gallop_right`](Self::gallop_right) over `tmp[start..start + len]` with hint 0, taking `tmp` out so the
    /// comparator can be borrowed alongside it.
    fn gallop_right_tmp(&mut self, key: usize, start: usize, len: usize) -> Result<usize, SortAborted> {
        let tmp = mem::take(&mut self.tmp);
        let result = self.gallop_right(key, &tmp[start..start + len], 0);
        self.tmp = tmp;
        result
    }

    /// [`gallop_left`](Self::gallop_left) over `tmp[..len]` with hint `len - 1`; see
    /// [`gallop_right_tmp`](Self::gallop_right_tmp).
    fn gallop_left_tmp(&mut self, key: usize, len: usize) -> Result<usize, SortAborted> {
        let tmp = mem::take(&mut self.tmp);
        let result = self.gallop_left(key, &tmp[..len], len - 1);
        self.tmp = tmp;
        result
    }
}

/// Positions within [`MergeState::merge_lo`], named after CPython's locals (`pa`/`pb` are its `ssa`/`ssb`):
/// `pa` indexes scratch (run A), `dest` and `pb` index `ids`.
struct LoCursor {
    dest: usize,
    pa: usize,
    pb: usize,
    na: usize,
    nb: usize,
}

/// Positions within [`MergeState::merge_hi`], named as in [`LoCursor`]: `pb` indexes scratch (run B), `dest` and
/// `pa` index `ids`. Each sits one past the next element, where CPython's pointers sit on it.
struct HiCursor {
    dest: usize,
    pa: usize,
    pb: usize,
    na: usize,
    nb: usize,
}

/// Reverses the group of `*neq + 1` equal elements ending just before `a[n]`, if there is one, and resets `*neq`.
fn reverse_last_equal<T>(a: &mut [T], n: usize, neq: &mut usize) {
    if *neq > 0 {
        let len = *neq + 1;
        a[n - len..n].reverse();
        *neq = 0;
    }
}

/// Returns the powersort level of the boundary between adjacent runs `[s1, s1 + n1)` and `[s1 + n1, s1 + n1 + n2)`
/// in a list of length `n`: the first bit at which the scaled midpoints of the two runs differ.
fn powerloop(s1: usize, n1: usize, n2: usize, n: usize) -> u32 {
    let mut result = 0;
    // Twice the midpoints, so they stay integers.
    let mut a = 2 * s1 + n1;
    let mut b = a + n1 + n2;
    loop {
        result += 1;
        if a >= n {
            a -= n;
            b -= n;
        } else if b >= n {
            break;
        }
        a <<= 1;
        b <<= 1;
    }
    result
}

/// Picks a minimum run length in `32..=64` (or `n` itself when smaller) so `n / minrun` is a power of two or just
/// under one, which keeps the final merges balanced.
fn merge_compute_minrun(mut n: usize) -> usize {
    let mut r = 0;
    while n >= MAX_MINRUN {
        r |= n & 1;
        n >>= 1;
    }
    n + r
}
