//! Fuzzy matching for pickers, the command palette and the jump list: every word of the
//! query must appear in the label as a subsequence (case-insensitive). Matches score higher
//! when they are contiguous and start words, and the matched characters are highlighted.

/// A match: its score (higher is better) and the matched char positions in the label.
#[derive(Debug, PartialEq, Eq)]
pub struct Match {
    pub score: i32,
    pub positions: Vec<usize>,
}

/// One char for one char, so positions in the folded text are positions in the label. The
/// query is folded the same way: lowercasing it as a whole string can give other chars
/// ("İ" becomes two) that would then never match the label.
fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn is_boundary(prev: Option<char>) -> bool {
    match prev {
        None => true,
        Some(c) => !c.is_alphanumeric(),
    }
}

/// Match one query word against the label chars, starting the search at every possible
/// first char and keeping the best-scoring greedy run.
fn word(label: &[char], lower: &[char], w: &[char]) -> Option<(i32, Vec<usize>)> {
    let mut best: Option<(i32, Vec<usize>)> = None;
    for start in 0..lower.len() {
        if lower[start] != w[0] {
            continue;
        }
        let mut pos = vec![start];
        let mut i = start + 1;
        for c in &w[1..] {
            while i < lower.len() && lower[i] != *c {
                i += 1;
            }
            if i == lower.len() {
                break;
            }
            pos.push(i);
            i += 1;
        }
        if pos.len() < w.len() {
            // later starts can only find fewer chars
            break;
        }
        let mut score = 0;
        for (n, p) in pos.iter().enumerate() {
            score += 1;
            if is_boundary(p.checked_sub(1).map(|q| label[q])) {
                score += 8;
            }
            if n > 0 && pos[n - 1] + 1 == *p {
                score += 5;
            }
        }
        // a match near the start of the label counts a little more
        score -= (start as i32).min(20) / 4;
        if best.as_ref().is_none_or(|(s, _)| score > *s) {
            best = Some((score, pos));
        }
    }
    best
}

/// Match `query` against `label`; `None` when some word does not match. An empty query
/// matches everything with score 0.
pub fn score(label: &str, query: &str) -> Option<Match> {
    let chars: Vec<char> = label.chars().collect();
    let lower: Vec<char> = chars.iter().copied().map(fold).collect();
    let mut total = 0;
    let mut positions = vec![];
    for w in query.split_whitespace() {
        let w: Vec<char> = w.chars().map(fold).collect();
        let (s, p) = word(&chars, &lower, &w)?;
        total += s;
        positions.extend(p);
    }
    positions.sort_unstable();
    positions.dedup();
    Some(Match { score: total, positions })
}

/// The items matching `query`, best first (ties keep the original order), with the
/// matched positions of each label.
pub fn filter<'a>(items: &'a [(String, String)], query: &str) -> Vec<(&'a (String, String), Vec<usize>)> {
    let mut out: Vec<_> = items.iter().filter_map(|it| score(&it.1, query).map(|m| (m.score, it, m.positions))).collect();
    // a stable sort: ties keep the original order
    out.sort_by_key(|(score, _, _)| std::cmp::Reverse(*score));
    out.into_iter().map(|(_, it, p)| (it, p)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(labels: &[&str]) -> Vec<(String, String)> {
        labels.iter().enumerate().map(|(i, l)| (i.to_string(), l.to_string())).collect()
    }

    #[test]
    fn subsequences_match() {
        assert!(score("hubness server", "hbs").is_some());
        assert!(score("hubness server", "srv hub").is_some());
        assert!(score("hubness", "xyz").is_none());
        assert_eq!(score("anything", ""), Some(Match { score: 0, positions: vec![] }));
        assert_eq!(score("Go to projects", "proj").unwrap().positions, vec![6, 7, 8, 9]);
    }

    #[test]
    fn word_starts_and_runs_rank_first() {
        let list = items(&["approve", "go to projects", "harness"]);
        let shown: Vec<&str> = filter(&list, "pro").iter().map(|(it, _)| it.1.as_str()).collect();
        assert_eq!(shown[0], "go to projects");
        assert_eq!(shown, vec!["go to projects", "approve"]);
    }

    #[test]
    fn empty_query_keeps_order() {
        let list = items(&["b", "a", "c"]);
        let shown: Vec<&str> = filter(&list, "").iter().map(|(it, _)| it.1.as_str()).collect();
        assert_eq!(shown, vec!["b", "a", "c"]);
    }

    #[test]
    fn case_and_unicode() {
        assert!(score("Ünïcode Título", "título").is_some());
        assert!(score("README.md", "readme").is_some());
        // the label and the query fold the same way, whatever case either is typed in
        assert!(score("İstanbul office", "İst").is_some());
        assert!(score("İstanbul office", "ist").is_some());
        assert!(score("istanbul", "İST").is_some());
        assert!(score("Straße", "STRASSE").is_none(), "ß is one letter here, not ss");
        assert!(score("Straße", "straße").is_some());
        assert!(score("Проект Альфа", "альфа про").is_some());
        // positions are char positions, not bytes
        assert_eq!(score("Проект Альфа", "аль").unwrap().positions, vec![7, 8, 9]);
    }

    #[test]
    fn words_match_in_any_order_and_all_must_match() {
        assert!(score("hubness server", "server hub").is_some());
        assert!(score("hubness server", "server zzz").is_none(), "one word that doesn't match fails the label");
        assert_eq!(score("hubness", "   "), Some(Match { score: 0, positions: vec![] }), "blank is empty");
        assert_eq!(score("", "").map(|m| m.score), Some(0));
        assert!(score("", "a").is_none());
        // two words landing on the same chars: each position is listed once, in order
        let m = score("abc", "ab bc").unwrap();
        assert_eq!(m.positions, vec![0, 1, 2]);
        // a longer query than the label never matches (and never panics)
        assert!(score("ab", "abc").is_none());
    }

    #[test]
    fn ties_keep_the_original_order() {
        let list = items(&["alpha one", "alpha two", "alpha three", "beta"]);
        let shown: Vec<&str> = filter(&list, "alpha").iter().map(|(it, _)| it.1.as_str()).collect();
        assert_eq!(shown, vec!["alpha one", "alpha two", "alpha three"]);
        // the positions come with each item, for the highlight
        let first = &filter(&list, "alpha t")[0];
        assert_eq!(first.0.1, "alpha two");
        assert_eq!(first.1, vec![0, 1, 2, 3, 4, 6]);
        assert!(filter(&list, "zzz").is_empty());
        assert!(filter(&[], "a").is_empty());
    }
}
