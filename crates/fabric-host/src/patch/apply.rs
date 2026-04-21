use super::matching::{find_subsequence, find_subsequence_fuzzy_unique};
use super::types::{PatchHunk, PatchHunkLine};

pub(crate) fn apply_update_hunks(content: &str, hunks: &[PatchHunk]) -> Result<String, String> {
    let mut lines = split_content_lines(content);
    let had_trailing_newline = content.ends_with('\n');
    let mut search_from = 0usize;

    for hunk in hunks {
        let (old_lines, new_lines) = hunk_old_new_lines(hunk);
        if old_lines.is_empty() {
            let insert_at = search_from.min(lines.len());
            lines.splice(insert_at..insert_at, new_lines.clone());
            search_from = insert_at + new_lines.len();
            continue;
        }

        let position = if let Some(index) = find_subsequence(&lines, &old_lines, search_from)
            .or_else(|| find_subsequence(&lines, &old_lines, 0))
        {
            index
        } else {
            match find_subsequence_fuzzy_unique(&lines, &old_lines, search_from) {
                Ok(Some(index)) => index,
                Ok(None) => {
                    return Err(format!(
                        "failed to match hunk '{}' (exact and fuzzy matching failed)",
                        hunk.header
                    ));
                }
                Err(matches) => {
                    return Err(format!(
                        "failed to match hunk '{}': fuzzy match is ambiguous ({} candidates)",
                        hunk.header, matches
                    ));
                }
            }
        };

        let end = position + old_lines.len();
        lines.splice(position..end, new_lines.clone());
        search_from = position + new_lines.len();
    }

    let mut updated = lines.join("\n");
    if had_trailing_newline {
        updated.push('\n');
    }
    Ok(updated)
}

fn split_content_lines(content: &str) -> Vec<String> {
    if content.is_empty() {
        return Vec::new();
    }

    let mut lines: Vec<String> = content.split('\n').map(str::to_string).collect();
    if content.ends_with('\n') && lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

fn hunk_old_new_lines(hunk: &PatchHunk) -> (Vec<String>, Vec<String>) {
    let mut old_lines = Vec::new();
    let mut new_lines = Vec::new();
    for line in &hunk.lines {
        match line {
            PatchHunkLine::Context(value) => {
                old_lines.push(value.clone());
                new_lines.push(value.clone());
            }
            PatchHunkLine::Delete(value) => old_lines.push(value.clone()),
            PatchHunkLine::Add(value) => new_lines.push(value.clone()),
            PatchHunkLine::EndOfFile => {}
        }
    }
    (old_lines, new_lines)
}

#[cfg(test)]
mod tests {
    use super::apply_update_hunks;
    use crate::patch::types::{PatchHunk, PatchHunkLine};

    fn hunk(header: &str, lines: Vec<PatchHunkLine>) -> PatchHunk {
        PatchHunk {
            header: header.to_owned(),
            lines,
        }
    }

    #[test]
    fn applies_context_based_update_and_preserves_trailing_newline() {
        let updated = apply_update_hunks(
            "fn main() {\n    println!(\"Hello\");\n    return 0;\n}\n",
            &[hunk(
                "@@ fn main()",
                vec![
                    PatchHunkLine::Context("    println!(\"Hello\");".to_owned()),
                    PatchHunkLine::Delete("    return 0;".to_owned()),
                    PatchHunkLine::Add("    println!(\"World\");".to_owned()),
                    PatchHunkLine::Add("    return 1;".to_owned()),
                ],
            )],
        )
        .unwrap();

        assert_eq!(
            updated,
            "fn main() {\n    println!(\"Hello\");\n    println!(\"World\");\n    return 1;\n}\n"
        );
    }

    #[test]
    fn applies_multiple_hunks_in_order() {
        let updated = apply_update_hunks(
            "DEFAULT_TIMEOUT = 30\n\ndef load_config():\n    config = {}\n    config[\"debug\"] = False\n",
            &[
                hunk(
                    "@@ DEFAULT_TIMEOUT = 30",
                    vec![
                        PatchHunkLine::Delete("DEFAULT_TIMEOUT = 30".to_owned()),
                        PatchHunkLine::Add("DEFAULT_TIMEOUT = 60".to_owned()),
                    ],
                ),
                hunk(
                    "@@ def load_config():",
                    vec![
                        PatchHunkLine::Context("    config = {}".to_owned()),
                        PatchHunkLine::Delete("    config[\"debug\"] = False".to_owned()),
                        PatchHunkLine::Add("    config[\"debug\"] = True".to_owned()),
                    ],
                ),
            ],
        )
        .unwrap();

        assert_eq!(
            updated,
            "DEFAULT_TIMEOUT = 60\n\ndef load_config():\n    config = {}\n    config[\"debug\"] = True\n"
        );
    }

    #[test]
    fn inserts_when_hunk_has_only_added_lines() {
        let updated = apply_update_hunks(
            "alpha\nomega\n",
            &[hunk(
                "@@ insert",
                vec![
                    PatchHunkLine::Add("inserted one".to_owned()),
                    PatchHunkLine::Add("inserted two".to_owned()),
                ],
            )],
        )
        .unwrap();

        assert_eq!(updated, "inserted one\ninserted two\nalpha\nomega\n");
    }

    #[test]
    fn ignores_end_of_file_marker_during_apply() {
        let updated = apply_update_hunks(
            "old\n",
            &[hunk(
                "@@ eof",
                vec![
                    PatchHunkLine::Delete("old".to_owned()),
                    PatchHunkLine::Add("new".to_owned()),
                    PatchHunkLine::EndOfFile,
                ],
            )],
        )
        .unwrap();

        assert_eq!(updated, "new\n");
    }

    #[test]
    fn fuzzy_matches_whitespace_and_unicode_punctuation() {
        let updated = apply_update_hunks(
            "println!(\u{201c}hello\u{201d});\nlet  value = 1;\n",
            &[hunk(
                "@@ fuzzy",
                vec![
                    PatchHunkLine::Context("println!(\"hello\");".to_owned()),
                    PatchHunkLine::Delete("let value = 1;".to_owned()),
                    PatchHunkLine::Add("let value = 2;".to_owned()),
                ],
            )],
        )
        .unwrap();

        assert_eq!(updated, "println!(\"hello\");\nlet value = 2;\n");
    }

    #[test]
    fn reports_no_match_when_hunk_cannot_be_located() {
        let error = apply_update_hunks(
            "alpha\nbeta\n",
            &[hunk(
                "@@ missing",
                vec![
                    PatchHunkLine::Delete("gamma".to_owned()),
                    PatchHunkLine::Add("delta".to_owned()),
                ],
            )],
        )
        .unwrap_err();

        assert!(error.contains("failed to match hunk"));
    }

    #[test]
    fn reports_ambiguous_fuzzy_match() {
        let error = apply_update_hunks(
            "a  b\nx\na\tb\n",
            &[hunk(
                "@@ ambiguous",
                vec![
                    PatchHunkLine::Delete("a b".to_owned()),
                    PatchHunkLine::Add("z".to_owned()),
                ],
            )],
        )
        .unwrap_err();

        assert!(error.contains("ambiguous"));
    }

    #[test]
    fn preserves_missing_trailing_newline() {
        let updated = apply_update_hunks(
            "old",
            &[hunk(
                "@@ no newline",
                vec![
                    PatchHunkLine::Delete("old".to_owned()),
                    PatchHunkLine::Add("new".to_owned()),
                ],
            )],
        )
        .unwrap();

        assert_eq!(updated, "new");
    }
}
