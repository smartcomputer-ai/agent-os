pub(crate) fn find_subsequence(
    haystack: &[String],
    needle: &[String],
    start: usize,
) -> Option<usize> {
    if needle.is_empty() {
        return Some(start.min(haystack.len()));
    }
    if start >= haystack.len() || needle.len() > haystack.len() {
        return None;
    }

    let limit = haystack.len().saturating_sub(needle.len());
    for idx in start..=limit {
        if haystack[idx..idx + needle.len()] == *needle {
            return Some(idx);
        }
    }
    None
}

pub(crate) fn find_subsequence_fuzzy_unique(
    haystack: &[String],
    needle: &[String],
    start: usize,
) -> Result<Option<usize>, usize> {
    if needle.is_empty() {
        return Ok(Some(start.min(haystack.len())));
    }
    if needle.len() > haystack.len() {
        return Ok(None);
    }

    let normalized_haystack: Vec<String> =
        haystack.iter().map(|line| normalize_line(line)).collect();
    let normalized_needle: Vec<String> = needle.iter().map(|line| normalize_line(line)).collect();

    let mut matches = Vec::new();
    let limit = normalized_haystack
        .len()
        .saturating_sub(normalized_needle.len());
    let begin = start.min(limit.saturating_add(1));
    for idx in begin..=limit {
        if normalized_haystack[idx..idx + normalized_needle.len()] == *normalized_needle {
            matches.push(idx);
            if matches.len() > 32 {
                break;
            }
        }
    }

    if matches.is_empty() && begin != 0 {
        for idx in 0..=limit {
            if normalized_haystack[idx..idx + normalized_needle.len()] == *normalized_needle {
                matches.push(idx);
                if matches.len() > 32 {
                    break;
                }
            }
        }
    }

    match matches.len() {
        0 => Ok(None),
        1 => Ok(Some(matches[0])),
        n => Err(n),
    }
}

pub(crate) fn normalize_line(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut saw_whitespace = false;

    for ch in input.chars() {
        if ch.is_whitespace() {
            saw_whitespace = true;
            continue;
        }

        if saw_whitespace && !result.is_empty() {
            result.push(' ');
        }
        saw_whitespace = false;
        result.push(canonical_char(ch));
    }

    result
}

fn canonical_char(ch: char) -> char {
    match ch {
        '\u{2018}' | '\u{2019}' | '\u{02BC}' => '\'',
        '\u{201C}' | '\u{201D}' => '"',
        '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2212}' => '-',
        _ => ch,
    }
}

#[cfg(test)]
mod tests {
    use super::{find_subsequence, find_subsequence_fuzzy_unique, normalize_line};

    #[test]
    fn normalize_line_collapses_whitespace_and_unicode_punctuation() {
        let normalized = normalize_line("  a\t\u{2018}b\u{2019}  \u{2014}  c  ");
        assert_eq!(normalized, "a 'b' - c");
    }

    #[test]
    fn find_subsequence_returns_exact_index() {
        let haystack = vec![
            "one".to_string(),
            "two".to_string(),
            "three".to_string(),
            "four".to_string(),
        ];
        let needle = vec!["two".to_string(), "three".to_string()];

        assert_eq!(find_subsequence(&haystack, &needle, 0), Some(1));
    }

    #[test]
    fn find_subsequence_fuzzy_unique_matches_whitespace_variants() {
        let haystack = vec!["fn  main() {".to_string(), "x".to_string()];
        let needle = vec!["fn main() {".to_string()];

        let found = find_subsequence_fuzzy_unique(&haystack, &needle, 0)
            .expect("fuzzy result should not be ambiguous");
        assert_eq!(found, Some(0));
    }

    #[test]
    fn find_subsequence_fuzzy_unique_reports_ambiguity() {
        let haystack = vec!["a  b".to_string(), "x".to_string(), "a b".to_string()];
        let needle = vec!["a b".to_string()];

        let err = find_subsequence_fuzzy_unique(&haystack, &needle, 0)
            .expect_err("expected ambiguous fuzzy match");
        assert_eq!(err, 2);
    }
}
