//! Fuzzy path matching.
//!
//! Typing `sesman` should find `crates/session-manager/src/lib.rs`. Every
//! subsequence match is a candidate, so the *ranking* is the whole feature — a
//! matcher that finds everything and orders it badly is no better than `grep`.
//!
//! ## Why dynamic programming rather than a greedy scan
//!
//! A greedy left-to-right scan finds *a* match but not the *best* one: given
//! `pt` against `PaneTree.tsx` it happily takes the `t` from `.tsx` instead of the
//! one starting `Tree`. Trying to repair that afterwards by sliding positions
//! around does not work either — sliding each position as far right as it will go
//! maximises spread, which is the opposite of what is wanted.
//!
//! So alignment is solved properly: `best[i][j]` is the score of the best match of
//! the first `i+1` query characters ending exactly at candidate position `j`. The
//! answer is the highest final value, and the positions come from a predecessor
//! table. Query strings are short and candidates are capped, so the cost is
//! irrelevant next to walking the filesystem.
//!
//! ## The heuristics
//!
//! - **A run inherits the bonus of the character that started it.** This is the
//!   single most important rule: it makes `ses` matching the start of `session`
//!   worth far more than the same letters landing mid-word, and it compounds
//!   across the run rather than being awarded once.
//! - **Gaps cost, and the first gap costs most.** One break is a much weaker
//!   signal than none; further breaks are incrementally cheaper so a long path
//!   is not ruled out entirely.
//! - **The basename beats the directory.** People search for a file.
//! - **Shorter and shallower wins ties.**
//!
//! Matching is case-insensitive until the query contains an uppercase letter —
//! "smart case", so convenience is the default and precision is available.

/// A scored match, with the positions that matched so the UI can highlight them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub score: i32,
    /// Character indices in the candidate that matched, ascending.
    pub positions: Vec<usize>,
}

// Weights. Relative magnitudes matter; absolute values do not.
const SCORE_MATCH: i32 = 16;
/// Deliberately larger than any single positional bonus, so a tight run always
/// beats a scattered one that happens to land on separators.
const BONUS_CONSECUTIVE: i32 = 18;
const BONUS_BOUNDARY: i32 = 10;
const BONUS_CAMEL: i32 = 9;
const BONUS_BASENAME: i32 = 4;
/// The first character of the basename is the strongest signal available.
const BONUS_FIRST_CHAR: i32 = 12;
/// Opening a gap costs; widening one costs less.
///
/// Kept deliberately mild. Harsher values made a tight run win — which is
/// correct — but also destroyed initial-style matches like `sm` against
/// `session_manager`, which is one of the most common ways people search paths.
const PENALTY_GAP_START: i32 = -5;
const PENALTY_GAP_EXTEND: i32 = -1;
const PENALTY_DEPTH: i32 = -2;

/// Compare two characters under the smart-case rule.
///
/// ASCII is handled without allocating. `char::to_lowercase` returns an iterator
/// because one character can lowercase to several, and paying for that on every cell
/// of the DP dominated the inner loop — while nearly every path in a repository is
/// ASCII.
#[inline]
fn chars_eq(a: char, b: char, case_sensitive: bool) -> bool {
    if case_sensitive {
        return a == b;
    }
    if a.is_ascii() && b.is_ascii() {
        return a.eq_ignore_ascii_case(&b);
    }
    a.to_lowercase().eq(b.to_lowercase())
}

/// Whether `query` appears in `candidate` in order, ignoring case.
///
/// The prefilter in front of the dynamic program. Operates on the strings directly so
/// it allocates nothing, which is the whole point: it runs for every candidate on every
/// keystroke, and the DP runs only for those that survive it.
fn is_subsequence(query: &str, candidate: &str) -> bool {
    let case_sensitive = query.chars().any(char::is_uppercase);
    let mut wanted = query.chars();
    let Some(mut needle) = wanted.next() else {
        return true;
    };
    for c in candidate.chars() {
        if chars_eq(needle, c, case_sensitive) {
            match wanted.next() {
                Some(next) => needle = next,
                // Every query character was found, in order.
                None => return true,
            }
        }
    }
    false
}

/// Longest candidate worth scoring.
const MAX_CANDIDATE: usize = 1024;
/// Longest query worth scoring. Beyond this it is not a fuzzy search.
const MAX_QUERY: usize = 64;

/// Reusable scratch space for scoring.
///
/// Scoring one candidate needs six buffers. Allocating them per candidate is a fixed
/// cost paid tens of thousands of times per keystroke, and for a short query it
/// dominates the dynamic program it exists to serve — so a caller ranking a corpus
/// keeps one of these and reuses it. [`score`] wraps it for one-off use.
#[derive(Default)]
pub struct Matcher {
    cand: Vec<char>,
    q: Vec<char>,
    bonuses: Vec<i32>,
    best: Vec<i32>,
    run: Vec<i32>,
    from: Vec<usize>,
}

impl Matcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Score `query` against `candidate`, or `None` if it does not match.
    ///
    /// `candidate` is expected to use `/` separators.
    pub fn score(&mut self, query: &str, candidate: &str) -> Option<Match> {
        score_with(self, query, candidate)
    }
}

/// Score `query` against `candidate`, or `None` if it does not match.
///
/// `candidate` is expected to use `/` separators. Allocates its scratch space; use
/// [`Matcher`] when scoring many candidates.
pub fn score(query: &str, candidate: &str) -> Option<Match> {
    score_with(&mut Matcher::new(), query, candidate)
}

fn score_with(m: &mut Matcher, query: &str, candidate: &str) -> Option<Match> {
    if query.is_empty() {
        return Some(Match {
            score: 0,
            positions: Vec::new(),
        });
    }

    // Reject before allocating anything.
    //
    // The dynamic program below costs O(query x candidate) and three allocations, and
    // for a typical query most candidates cannot match at all. A greedy forward scan
    // settles that in one pass over `candidate` with no allocation.
    //
    // Greedy is *complete* for existence even though it is not optimal for scoring: an
    // alignment exists only if the query is a subsequence, and if one exists the
    // leftmost-first scan finds one. So this can never reject a candidate the DP would
    // have matched — which is the property that makes the shortcut safe rather than
    // merely fast.
    if !is_subsequence(query, candidate) {
        return None;
    }

    // Refilled rather than reallocated. `clear` keeps the capacity, so after the first
    // candidate these are writes into memory that already exists.
    let Matcher {
        cand,
        q,
        bonuses,
        best,
        run,
        from,
    } = m;
    cand.clear();
    cand.extend(candidate.chars());
    q.clear();
    q.extend(query.chars());

    if cand.is_empty() || cand.len() > MAX_CANDIDATE || q.len() > MAX_QUERY || q.len() > cand.len()
    {
        return None;
    }

    let case_sensitive = q.iter().any(|c| c.is_uppercase());

    // Where the basename begins.
    let basename_start = cand
        .iter()
        .rposition(|&c| c == '/')
        .map(|i| i + 1)
        .unwrap_or(0);

    // Positional bonus for each candidate index, independent of the query.
    bonuses.clear();
    bonuses.extend((0..cand.len()).map(|i| positional_bonus(cand, i, basename_start)));

    // best[i][j]: score of the best alignment of q[0..=i] ending exactly at j.
    // run[i][j]: length of the consecutive run ending at j, used to compound the
    // starting bonus across the run.
    let n = cand.len();
    let m = q.len();
    // `clear` then `resize` rather than `resize` alone: the latter would leave stale
    // values from a longer previous candidate in the leading cells.
    best.clear();
    best.resize(n * m, i32::MIN);
    run.clear();
    run.resize(n * m, 0);
    from.clear();
    from.resize(n * m, usize::MAX);

    // Indexed rather than iterated: the body reads `q[i]` while writing `best`,
    // `run`, and `from` at `i * n + j`, so the index is the subject of the loop.
    // Iterators would need three parallel zips and would obscure the recurrence.
    #[allow(clippy::needless_range_loop)]
    for i in 0..m {
        for j in 0..n {
            if !chars_eq(q[i], cand[j], case_sensitive) {
                continue;
            }
            let idx = i * n + j;

            if i == 0 {
                // The first query character: pay for everything skipped before it,
                // so a match near the start of the basename is preferred.
                let skipped = j.saturating_sub(basename_start.min(j));
                best[idx] = SCORE_MATCH + bonuses[j] + gap_cost(skipped);
                run[idx] = 1;
                continue;
            }

            // Extend from any earlier position where the previous query character
            // matched.
            for k in 0..j {
                let prev = (i - 1) * n + k;
                if best[prev] == i32::MIN {
                    continue;
                }

                let consecutive = k + 1 == j;
                let (bonus, this_run) = if consecutive {
                    // A run keeps the bonus of the character that started it, and
                    // adds the consecutive bonus for every step.
                    (BONUS_CONSECUTIVE, run[prev] + 1)
                } else {
                    (bonuses[j], 1)
                };

                let gap = if consecutive { 0 } else { j - k - 1 };
                let candidate_score = best[prev] + SCORE_MATCH + bonus + gap_cost(gap);

                if candidate_score > best[idx] {
                    best[idx] = candidate_score;
                    run[idx] = this_run;
                    from[idx] = k;
                }
            }
        }
    }

    // The best complete alignment ends at whichever position scores highest.
    let last = m - 1;
    let mut end = usize::MAX;
    let mut total = i32::MIN;
    for j in 0..n {
        let idx = last * n + j;
        if best[idx] > total {
            total = best[idx];
            end = j;
        }
    }
    if end == usize::MAX {
        return None;
    }

    // Walk the predecessor table back to recover the positions.
    let mut positions = vec![0usize; m];
    let mut j = end;
    for i in (0..m).rev() {
        positions[i] = j;
        if i > 0 {
            j = from[i * n + j];
            debug_assert!(j != usize::MAX, "predecessor chain broken");
        }
    }

    // Shallower and shorter wins ties.
    total += PENALTY_DEPTH * candidate.matches('/').count() as i32;
    total -= (cand.len().saturating_sub(m) / 12) as i32;

    Some(Match {
        score: total,
        positions,
    })
}

/// What a match at `index` is worth by virtue of where it sits.
fn positional_bonus(cand: &[char], index: usize, basename_start: usize) -> i32 {
    let mut bonus = 0;

    if index == basename_start {
        // Start of the filename: the strongest positional signal.
        bonus += BONUS_FIRST_CHAR;
    } else if index == 0 {
        bonus += BONUS_BOUNDARY;
    } else {
        let before = cand[index - 1];
        if is_separator(before) {
            bonus += BONUS_BOUNDARY;
        } else if before.is_lowercase() && cand[index].is_uppercase() {
            bonus += BONUS_CAMEL;
        }
    }

    if index >= basename_start {
        bonus += BONUS_BASENAME;
    }
    bonus
}

/// Cost of skipping `gap` characters.
fn gap_cost(gap: usize) -> i32 {
    match gap {
        0 => 0,
        // Opening a gap is what hurts; widening it is incremental, and capped so
        // one long jump cannot dominate the score.
        _ => PENALTY_GAP_START + PENALTY_GAP_EXTEND * ((gap - 1).min(10) as i32),
    }
}

fn is_separator(c: char) -> bool {
    matches!(c, '/' | '_' | '-' | '.' | ' ' | '\\')
}

/// Rank candidates against a query, best first, keeping at most `limit`.
///
/// Ties break on length then lexicographically, so results are stable between
/// calls — a list that reshuffles on every keystroke cannot be used.
pub fn rank<'a, I>(query: &str, candidates: I, limit: usize) -> Vec<(&'a str, Match)>
where
    I: IntoIterator<Item = &'a str>,
{
    // One matcher for the whole corpus: this is what turns six allocations per
    // candidate into six for the entire ranking pass.
    let mut matcher = Matcher::new();
    let mut scored: Vec<(&str, Match)> = candidates
        .into_iter()
        .filter_map(|candidate| matcher.score(query, candidate).map(|m| (candidate, m)))
        .collect();

    scored.sort_by(|a, b| {
        b.1.score
            .cmp(&a.1.score)
            .then_with(|| a.0.len().cmp(&b.0.len()))
            .then_with(|| a.0.cmp(b.0))
    });
    scored.truncate(limit);
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    fn best(query: &str, candidates: &[&str]) -> String {
        rank(query, candidates.iter().copied(), 10)
            .first()
            .map(|(c, _)| c.to_string())
            .unwrap_or_default()
    }

    /// The substring of `candidate` that actually matched.
    fn matched(query: &str, candidate: &str) -> String {
        let m = score(query, candidate).expect("no match");
        candidate
            .chars()
            .enumerate()
            .filter(|(i, _)| m.positions.contains(i))
            .map(|(_, c)| c)
            .collect()
    }

    #[test]
    fn matches_a_subsequence_and_rejects_a_non_subsequence() {
        assert!(score("abc", "a-b-c").is_some());
        assert!(score("abc", "xaybzc").is_some());
        assert!(score("abc", "acb").is_none());
        assert!(score("abcd", "abc").is_none());
    }

    #[test]
    fn an_empty_query_matches_everything() {
        assert_eq!(score("", "anything").unwrap().score, 0);
    }

    #[test]
    fn the_matched_characters_are_always_the_query() {
        // Whatever the alignment chose, it must spell the query — case-insensitively,
        // since a lowercase query legitimately matches uppercase characters.
        for (q, c) in [
            ("prof", "crates/agent/profile.rs"),
            ("pt", "PaneTree.tsx"),
            ("sesman", "crates/session-manager/src/lib.rs"),
            ("md", "docs/DESIGN.md"),
        ] {
            assert_eq!(
                matched(q, c).to_lowercase(),
                q.to_lowercase(),
                "alignment for {q:?} in {c:?}"
            );
        }
    }

    #[test]
    fn picks_camel_case_humps_over_a_later_extension_match() {
        // The greedy failure this DP exists to fix: `pt` must not take its `t`
        // from `.tsx` when `Tree` is available.
        let m = score("pt", "PaneTree.tsx").unwrap();
        assert_eq!(m.positions, vec![0, 4]);
    }

    #[test]
    fn pulls_a_match_onto_the_basename_run() {
        // `prof` must land on `profile`, not scatter across the whole path.
        let m = score("prof", "crates/agent/profile.rs").unwrap();
        assert_eq!(matched("prof", "crates/agent/profile.rs"), "prof");
        for pair in m.positions.windows(2) {
            assert_eq!(pair[1], pair[0] + 1, "positions should be consecutive");
        }
    }

    #[test]
    fn a_tight_run_beats_the_same_letters_on_separators() {
        // The regression that motivated raising the consecutive bonus above every
        // positional bonus: separator bonuses must not out-earn an exact run.
        let tight = score("prof", "profile.rs").unwrap().score;
        let scattered = score("prof", "p_r_o_f_x.rs").unwrap().score;
        assert!(
            tight > scattered,
            "tight {tight} should beat scattered {scattered}"
        );
    }

    #[test]
    fn prefers_a_match_in_the_basename() {
        assert_eq!(
            best(
                "profile",
                &["src/profiles/index.ts", "src/agent/profile.rs"]
            ),
            "src/agent/profile.rs"
        );
    }

    #[test]
    fn prefers_word_boundaries_over_mid_word() {
        let boundary = score("sm", "session_manager.rs").unwrap().score;
        let midword = score("sm", "somsmall.rs").unwrap().score;
        assert!(boundary > midword, "{boundary} should beat {midword}");
    }

    #[test]
    fn prefers_shallower_paths_on_a_tie() {
        assert_eq!(best("index", &["a/b/c/d/index.ts", "index.ts"]), "index.ts");
    }

    #[test]
    fn smart_case_is_insensitive_until_the_query_has_uppercase() {
        assert!(score("readme", "README.md").is_some());
        assert!(score("README", "readme.md").is_none());
        assert!(score("README", "README.md").is_some());
    }

    #[test]
    fn finds_a_hyphenated_crate_from_a_squashed_query() {
        // The motivating case: initials and fragments across a separator.
        let candidates = [
            "crates/session-manager/src/lib.rs",
            "crates/shell-integration/src/lib.rs",
            "ui/src/lib/store.ts",
        ];
        assert_eq!(
            best("sesman", &candidates),
            "crates/session-manager/src/lib.rs"
        );
    }

    #[test]
    fn distinguishes_similarly_named_crates() {
        let candidates = [
            "crates/session-manager/src/ssh.rs",
            "crates/shell-integration/src/aliases.rs",
        ];
        assert_eq!(
            best("ssh", &candidates),
            "crates/session-manager/src/ssh.rs"
        );
        assert_eq!(
            best("alias", &candidates),
            "crates/shell-integration/src/aliases.rs"
        );
    }

    #[test]
    fn positions_are_ascending_and_in_range() {
        let candidate = "crates/file-index/src/fuzzy.rs";
        let m = score("fizz", candidate).unwrap();
        assert!(m.positions.windows(2).all(|w| w[0] < w[1]));
        assert!(m.positions.iter().all(|&p| p < candidate.chars().count()));
    }

    #[test]
    fn ranking_is_stable_across_calls() {
        let candidates = ["a/x.rs", "b/x.rs", "c/x.rs"];
        let first = rank("x", candidates.iter().copied(), 10);
        let second = rank("x", candidates.iter().copied(), 10);
        assert_eq!(
            first.iter().map(|(c, _)| *c).collect::<Vec<_>>(),
            second.iter().map(|(c, _)| *c).collect::<Vec<_>>()
        );
    }

    #[test]
    fn ranking_respects_the_limit() {
        let candidates: Vec<String> = (0..100).map(|i| format!("file{i}.rs")).collect();
        let refs: Vec<&str> = candidates.iter().map(String::as_str).collect();
        assert_eq!(rank("file", refs, 7).len(), 7);
    }

    #[test]
    fn handles_multibyte_candidates_without_panicking() {
        assert!(score("日本", "docs/日本語.md").is_some());
        assert!(score("md", "docs/日本語.md").is_some());
        // Positions are character indices, so slicing by them is safe.
        let m = score("日本", "docs/日本語.md").unwrap();
        assert_eq!(m.positions.len(), 2);
    }

    #[test]
    fn rejects_input_that_is_not_a_fuzzy_search() {
        assert!(score("x", &"x".repeat(MAX_CANDIDATE + 1)).is_none());
        assert!(score(&"x".repeat(MAX_QUERY + 1), "xyz").is_none());
        assert!(score("", "").unwrap().score == 0);
    }

    #[test]
    fn scoring_a_large_candidate_set_is_fast_enough_to_type_against() {
        // Completion runs on every keystroke, so this is a correctness property of
        // the feature, not a micro-benchmark.
        let candidates: Vec<String> = (0..20_000)
            .map(|i| format!("crates/pkg{}/src/module_{i}.rs", i % 40))
            .collect();
        let refs: Vec<&str> = candidates.iter().map(String::as_str).collect();

        let start = std::time::Instant::now();
        let results = rank("pkgmod", refs, 50);
        let elapsed = start.elapsed();

        assert_eq!(results.len(), 50);
        assert!(
            elapsed < std::time::Duration::from_millis(400),
            "ranking 20k paths took {elapsed:?}"
        );
    }
}
