use super::types::{ParsedPatch, PatchHunk, PatchHunkLine, PatchOpCounts, PatchOperation};

pub(crate) fn parse_patch_v4a(patch: &str) -> Result<ParsedPatch, String> {
    let lines: Vec<&str> = patch.lines().collect();
    if lines.first().copied() != Some("*** Begin Patch") {
        return Err("apply_patch payload must start with '*** Begin Patch'".to_string());
    }
    if lines.last().copied() != Some("*** End Patch") {
        return Err("apply_patch payload must end with '*** End Patch'".to_string());
    }

    let mut operations = Vec::new();
    let mut counts = PatchOpCounts::default();
    let mut idx = 1usize;
    let end = lines.len().saturating_sub(1);
    while idx < end {
        let line = lines[idx];
        if line.trim().is_empty() {
            idx += 1;
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Add File: ") {
            idx += 1;
            let mut added = Vec::new();
            while idx < end && !is_patch_operation_start(lines[idx]) {
                let Some(payload) = lines[idx].strip_prefix('+') else {
                    return Err(format!("invalid add-file line: '{}'", lines[idx]));
                };
                added.push(payload.to_string());
                idx += 1;
            }
            operations.push(PatchOperation::AddFile {
                path: path.to_string(),
                lines: added,
            });
            counts.add += 1;
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Delete File: ") {
            operations.push(PatchOperation::DeleteFile {
                path: path.to_string(),
            });
            counts.delete += 1;
            idx += 1;
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Update File: ") {
            idx += 1;
            let mut move_to = None;
            if idx < end {
                if let Some(target) = lines[idx].strip_prefix("*** Move to: ") {
                    move_to = Some(target.to_string());
                    counts.move_count += 1;
                    idx += 1;
                }
            }

            let mut hunks = Vec::new();
            while idx < end && !is_patch_operation_start(lines[idx]) {
                let header = lines[idx];
                if !header.starts_with("@@") {
                    return Err(format!(
                        "invalid hunk header in update '{}': '{}'",
                        path, header
                    ));
                }
                idx += 1;

                let mut hunk_lines = Vec::new();
                while idx < end
                    && !is_patch_operation_start(lines[idx])
                    && !lines[idx].starts_with("@@")
                {
                    let hunk_line = lines[idx];
                    if hunk_line == "*** End of File" {
                        hunk_lines.push(PatchHunkLine::EndOfFile);
                        idx += 1;
                        continue;
                    }
                    let Some(prefix) = hunk_line.chars().next() else {
                        return Err("empty hunk line is not allowed".to_string());
                    };
                    let value = hunk_line[1..].to_string();
                    let parsed = match prefix {
                        ' ' => PatchHunkLine::Context(value),
                        '-' => PatchHunkLine::Delete(value),
                        '+' => PatchHunkLine::Add(value),
                        _ => {
                            return Err(format!(
                                "invalid hunk line prefix '{}' in '{}'",
                                prefix, hunk_line
                            ));
                        }
                    };
                    hunk_lines.push(parsed);
                    idx += 1;
                }

                if hunk_lines.is_empty() {
                    return Err(format!("empty hunk in update '{}'", path));
                }
                hunks.push(PatchHunk {
                    header: header.to_string(),
                    lines: hunk_lines,
                });
            }

            if hunks.is_empty() {
                return Err(format!(
                    "update operation for '{}' must include at least one hunk",
                    path
                ));
            }

            operations.push(PatchOperation::UpdateFile {
                path: path.to_string(),
                move_to,
                hunks,
            });
            counts.update += 1;
            continue;
        }

        return Err(format!("unknown patch operation line: '{}'", line));
    }

    if operations.is_empty() {
        return Err("patch must contain at least one operation".to_string());
    }

    Ok(ParsedPatch { operations, counts })
}

fn is_patch_operation_start(line: &str) -> bool {
    line.starts_with("*** Add File: ")
        || line.starts_with("*** Delete File: ")
        || line.starts_with("*** Update File: ")
}

#[cfg(test)]
mod tests {
    use super::parse_patch_v4a;
    use crate::patch::types::{PatchHunkLine, PatchOperation};

    #[test]
    fn parses_add_file_operation() {
        let parsed = parse_patch_v4a(
            "\
*** Begin Patch
*** Add File: src/utils/helpers.py
+def greet(name):
+    return f\"Hello, {name}!\"
*** End Patch",
        )
        .unwrap();

        assert_eq!(parsed.counts.add, 1);
        assert_eq!(parsed.counts.update, 0);
        assert_eq!(parsed.counts.delete, 0);
        assert_eq!(parsed.counts.move_count, 0);
        assert_eq!(
            parsed.operations,
            vec![PatchOperation::AddFile {
                path: "src/utils/helpers.py".to_owned(),
                lines: vec![
                    "def greet(name):".to_owned(),
                    "    return f\"Hello, {name}!\"".to_owned(),
                ],
            }]
        );
    }

    #[test]
    fn parses_delete_file_operation() {
        let parsed = parse_patch_v4a(
            "\
*** Begin Patch
*** Delete File: src/old_module.py
*** End Patch",
        )
        .unwrap();

        assert_eq!(parsed.counts.delete, 1);
        assert_eq!(
            parsed.operations,
            vec![PatchOperation::DeleteFile {
                path: "src/old_module.py".to_owned(),
            }]
        );
    }

    #[test]
    fn parses_update_with_context_delete_add_and_eof_marker() {
        let parsed = parse_patch_v4a(
            "\
*** Begin Patch
*** Update File: src/main.py
@@ def main():
     print(\"Hello\")
-    return 0
+    print(\"World\")
+    return 1
*** End of File
*** End Patch",
        )
        .unwrap();

        assert_eq!(parsed.counts.update, 1);
        let PatchOperation::UpdateFile {
            path,
            move_to,
            hunks,
        } = &parsed.operations[0]
        else {
            panic!("expected update operation");
        };
        assert_eq!(path, "src/main.py");
        assert_eq!(move_to, &None);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].header, "@@ def main():");
        assert_eq!(
            hunks[0].lines,
            vec![
                PatchHunkLine::Context("    print(\"Hello\")".to_owned()),
                PatchHunkLine::Delete("    return 0".to_owned()),
                PatchHunkLine::Add("    print(\"World\")".to_owned()),
                PatchHunkLine::Add("    return 1".to_owned()),
                PatchHunkLine::EndOfFile,
            ]
        );
    }

    #[test]
    fn parses_update_with_move_and_multiple_hunks() {
        let parsed = parse_patch_v4a(
            "\
*** Begin Patch
*** Update File: old_name.py
*** Move to: new_name.py
@@ import os
 import sys
-import old_dep
+import new_dep
@@ def load_config():
     config = {}
-    config[\"debug\"] = False
+    config[\"debug\"] = True
*** End Patch",
        )
        .unwrap();

        assert_eq!(parsed.counts.update, 1);
        assert_eq!(parsed.counts.move_count, 1);
        let PatchOperation::UpdateFile {
            path,
            move_to,
            hunks,
        } = &parsed.operations[0]
        else {
            panic!("expected update operation");
        };
        assert_eq!(path, "old_name.py");
        assert_eq!(move_to.as_deref(), Some("new_name.py"));
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].header, "@@ import os");
        assert_eq!(hunks[1].header, "@@ def load_config():");
    }

    #[test]
    fn parses_mixed_operations_and_counts_them() {
        let parsed = parse_patch_v4a(
            "\
*** Begin Patch
*** Add File: added.txt
+added
*** Update File: existing.txt
@@
-old
+new
*** Delete File: removed.txt
*** End Patch",
        )
        .unwrap();

        assert_eq!(parsed.operations.len(), 3);
        assert_eq!(parsed.counts.add, 1);
        assert_eq!(parsed.counts.update, 1);
        assert_eq!(parsed.counts.delete, 1);
        assert_eq!(parsed.counts.move_count, 0);
    }

    #[test]
    fn rejects_payload_without_begin_marker() {
        let error = parse_patch_v4a("*** Add File: a\n+x\n*** End Patch").unwrap_err();
        assert!(error.contains("must start"));
    }

    #[test]
    fn rejects_payload_without_end_marker() {
        let error = parse_patch_v4a("*** Begin Patch\n*** Add File: a\n+x").unwrap_err();
        assert!(error.contains("must end"));
    }

    #[test]
    fn rejects_empty_patch() {
        let error = parse_patch_v4a("*** Begin Patch\n*** End Patch").unwrap_err();
        assert!(error.contains("at least one operation"));
    }

    #[test]
    fn rejects_add_file_lines_without_plus_prefix() {
        let error = parse_patch_v4a(
            "\
*** Begin Patch
*** Add File: a.txt
missing plus
*** End Patch",
        )
        .unwrap_err();
        assert!(error.contains("invalid add-file line"));
    }

    #[test]
    fn rejects_update_without_hunks() {
        let error = parse_patch_v4a(
            "\
*** Begin Patch
*** Update File: a.txt
*** End Patch",
        )
        .unwrap_err();
        assert!(error.contains("must include at least one hunk"));
    }

    #[test]
    fn rejects_empty_hunk() {
        let error = parse_patch_v4a(
            "\
*** Begin Patch
*** Update File: a.txt
@@ first
@@ second
-old
+new
*** End Patch",
        )
        .unwrap_err();
        assert!(error.contains("empty hunk"));
    }

    #[test]
    fn rejects_invalid_hunk_line_prefix() {
        let error = parse_patch_v4a(
            "\
*** Begin Patch
*** Update File: a.txt
@@
?bad
*** End Patch",
        )
        .unwrap_err();
        assert!(error.contains("invalid hunk line prefix"));
    }
}
