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
    (start..=limit).find(|&idx| haystack[idx..idx + needle.len()] == *needle)
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

fn normalize_line(input: &str) -> String {
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
    use super::{find_subsequence, find_subsequence_fuzzy_unique};

    #[test]
    fn exact_subsequence_respects_start_offset() {
        let haystack = vec![
            "one".to_owned(),
            "two".to_owned(),
            "three".to_owned(),
            "two".to_owned(),
            "three".to_owned(),
        ];
        let needle = vec!["two".to_owned(), "three".to_owned()];

        assert_eq!(find_subsequence(&haystack, &needle, 0), Some(1));
        assert_eq!(find_subsequence(&haystack, &needle, 2), Some(3));
        assert_eq!(find_subsequence(&haystack, &needle, 4), None);
    }

    #[test]
    fn empty_needle_matches_at_start_offset_clamped_to_len() {
        let haystack = vec!["one".to_owned()];

        assert_eq!(find_subsequence(&haystack, &[], 0), Some(0));
        assert_eq!(find_subsequence(&haystack, &[], 99), Some(1));
    }

    #[test]
    fn fuzzy_match_collapses_whitespace() {
        let haystack = vec!["fn  main() {".to_owned(), "    ok()".to_owned()];
        let needle = vec!["fn main() {".to_owned(), "ok()".to_owned()];

        assert_eq!(
            find_subsequence_fuzzy_unique(&haystack, &needle, 0).unwrap(),
            Some(0)
        );
    }

    #[test]
    fn fuzzy_match_canonicalizes_unicode_punctuation() {
        let haystack = vec![
            "let quote = \u{201c}hello\u{201d};".to_owned(),
            "let dash = a \u{2014} b;".to_owned(),
        ];
        let needle = vec![
            "let quote = \"hello\";".to_owned(),
            "let dash = a - b;".to_owned(),
        ];

        assert_eq!(
            find_subsequence_fuzzy_unique(&haystack, &needle, 0).unwrap(),
            Some(0)
        );
    }

    #[test]
    fn fuzzy_match_falls_back_before_start_when_no_later_match_exists() {
        let haystack = vec!["target".to_owned(), "middle".to_owned(), "tail".to_owned()];
        let needle = vec!["target".to_owned()];

        assert_eq!(
            find_subsequence_fuzzy_unique(&haystack, &needle, 2).unwrap(),
            Some(0)
        );
    }

    #[test]
    fn fuzzy_match_reports_ambiguity() {
        let haystack = vec!["a  b".to_owned(), "x".to_owned(), "a b".to_owned()];
        let needle = vec!["a b".to_owned()];

        assert_eq!(
            find_subsequence_fuzzy_unique(&haystack, &needle, 0).unwrap_err(),
            2
        );
    }
}
