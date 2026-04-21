#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EditApplyResult {
    pub(crate) updated: String,
    pub(crate) replacements: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EditMatchError {
    NotFound,
    Ambiguous(usize),
}

pub(crate) fn apply_edit(
    content: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
) -> Result<EditApplyResult, EditMatchError> {
    let replacement_count = content.match_indices(old_string).count();
    if replacement_count > 0 {
        if replacement_count > 1 && !replace_all {
            return Err(EditMatchError::Ambiguous(replacement_count));
        }

        let updated = if replace_all {
            content.replace(old_string, new_string)
        } else {
            content.replacen(old_string, new_string, 1)
        };
        return Ok(EditApplyResult {
            updated,
            replacements: if replace_all { replacement_count } else { 1 },
        });
    }

    let fuzzy = fuzzy_match_spans(content, old_string);
    if fuzzy.is_empty() {
        return Err(EditMatchError::NotFound);
    }
    if fuzzy.len() > 1 && !replace_all {
        return Err(EditMatchError::Ambiguous(fuzzy.len()));
    }

    let spans = if replace_all { fuzzy } else { vec![fuzzy[0]] };
    let updated = replace_spans(content, &spans, new_string);
    Ok(EditApplyResult {
        updated,
        replacements: spans.len(),
    })
}

fn replace_spans(source: &str, spans: &[(usize, usize)], replacement: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut cursor = 0usize;
    for (start, end) in spans {
        out.push_str(&source[cursor..*start]);
        out.push_str(replacement);
        cursor = *end;
    }
    out.push_str(&source[cursor..]);
    out
}

fn fuzzy_match_spans(source: &str, needle: &str) -> Vec<(usize, usize)> {
    let (source_norm, source_map) = normalize_with_map(source);
    let (needle_norm, _) = normalize_with_map(needle);
    if needle_norm.is_empty() || source_norm.len() < needle_norm.len() {
        return Vec::new();
    }

    let mut spans = Vec::new();
    let mut last_end = 0usize;
    for start in 0..=source_norm.len() - needle_norm.len() {
        if source_norm[start..start + needle_norm.len()] == needle_norm {
            let start_byte = source_map[start];
            let end_byte = if start + needle_norm.len() < source_map.len() {
                source_map[start + needle_norm.len()]
            } else {
                source.len()
            };
            if start_byte >= last_end {
                spans.push((start_byte, end_byte));
                last_end = end_byte;
            }
        }
    }
    spans
}

fn normalize_with_map(input: &str) -> (Vec<char>, Vec<usize>) {
    let mut out = Vec::new();
    let mut map = Vec::new();
    let mut in_space = false;
    for (byte_idx, ch) in input.char_indices() {
        let mapped = match ch {
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
            '\u{2013}' | '\u{2014}' | '\u{2212}' => '-',
            _ => ch,
        };

        if mapped.is_whitespace() {
            if !in_space {
                out.push(' ');
                map.push(byte_idx);
                in_space = true;
            }
            continue;
        }

        in_space = false;
        out.push(mapped);
        map.push(byte_idx);
    }

    (out, map)
}

#[cfg(test)]
mod tests {
    use super::{EditApplyResult, EditMatchError, apply_edit};

    #[test]
    fn exact_match_replaces_once_by_default() {
        let result = apply_edit("a b\nc d\n", "a b", "x", false).unwrap();

        assert_eq!(
            result,
            EditApplyResult {
                updated: "x\nc d\n".to_owned(),
                replacements: 1,
            }
        );
    }

    #[test]
    fn exact_match_can_replace_all() {
        let result = apply_edit("a b\na b\n", "a b", "x", true).unwrap();

        assert_eq!(
            result,
            EditApplyResult {
                updated: "x\nx\n".to_owned(),
                replacements: 2,
            }
        );
    }

    #[test]
    fn exact_match_reports_ambiguity_without_replace_all() {
        let error = apply_edit("a b\na b\n", "a b", "x", false).unwrap_err();

        assert_eq!(error, EditMatchError::Ambiguous(2));
    }

    #[test]
    fn fuzzy_match_collapses_whitespace_and_unicode_punctuation() {
        let result = apply_edit(
            "println!(\u{201c}hello\u{201d});\nlet  value = 1;\n",
            "println!(\"hello\");\nlet value = 1;",
            "done();",
            false,
        )
        .unwrap();

        assert_eq!(
            result,
            EditApplyResult {
                updated: "done();\n".to_owned(),
                replacements: 1,
            }
        );
    }

    #[test]
    fn fuzzy_match_reports_not_found() {
        let error = apply_edit("alpha\nbeta\n", "gamma", "delta", false).unwrap_err();

        assert_eq!(error, EditMatchError::NotFound);
    }

    #[test]
    fn fuzzy_match_reports_ambiguity_without_replace_all() {
        let error = apply_edit("a  b\nx\na\tb\n", "a b", "z", false).unwrap_err();

        assert_eq!(error, EditMatchError::Ambiguous(2));
    }

    #[test]
    fn fuzzy_match_can_replace_all() {
        let result = apply_edit("a  b\nx\na\tb\n", "a b", "z", true).unwrap();

        assert_eq!(
            result,
            EditApplyResult {
                updated: "z\nx\nz\n".to_owned(),
                replacements: 2,
            }
        );
    }
}
