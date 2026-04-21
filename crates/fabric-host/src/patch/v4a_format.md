## Appendix A: apply_patch v4a Format Reference

The `apply_patch` tool (used by the OpenAI profile) accepts patches in the v4a format. This format supports creating, deleting, updating, and renaming files in a single patch.

### Grammar

```
patch       = "*** Begin Patch\n" operations "*** End Patch\n"
operations  = (add_file | delete_file | update_file)*

add_file    = "*** Add File: " path "\n" added_lines
delete_file = "*** Delete File: " path "\n"
update_file = "*** Update File: " path "\n" [move_line] hunks

move_line   = "*** Move to: " new_path "\n"
added_lines = ("+" line "\n")*
hunks       = hunk+
hunk        = "@@ " [context_hint] "\n" hunk_lines
hunk_lines  = (context_line | delete_line | add_line)+
context_line = " " line "\n"           -- space prefix = unchanged line
delete_line  = "-" line "\n"           -- minus prefix = remove this line
add_line     = "+" line "\n"           -- plus prefix = add this line
eof_marker   = "*** End of File\n"     -- optional, marks end of last hunk
```

### Operations

**Add File:** Creates a new file. All lines are prefixed with `+`.
```
*** Begin Patch
*** Add File: src/utils/helpers.py
+def greet(name):
+    return f"Hello, {name}!"
*** End Patch
```

**Delete File:** Removes a file entirely.
```
*** Begin Patch
*** Delete File: src/old_module.py
*** End Patch
```

**Update File:** Modifies an existing file using context-based hunks.
```
*** Begin Patch
*** Update File: src/main.py
@@ def main():
     print("Hello")
-    return 0
+    print("World")
+    return 1
*** End Patch
```

**Update + Rename:** Modify and rename in one operation.
```
*** Begin Patch
*** Update File: old_name.py
*** Move to: new_name.py
@@ import os
 import sys
-import old_dep
+import new_dep
*** End Patch
```

### Hunk Matching

The `@@` line provides a context hint (typically a function signature or recognizable line near the change). The implementation uses this hint plus the context lines (space-prefixed) to locate the correct position in the file. Convention: show 3 lines of context above and below each change.

When exact matching fails, the implementation should attempt fuzzy matching (whitespace normalization, Unicode punctuation equivalence) before reporting an error.

### Multi-Hunk Updates

A single Update File block can contain multiple `@@` hunks:

```
*** Begin Patch
*** Update File: src/config.py
@@ DEFAULT_TIMEOUT = 30
-DEFAULT_TIMEOUT = 30
+DEFAULT_TIMEOUT = 60
@@ def load_config():
     config = {}
-    config["debug"] = False
+    config["debug"] = True
*** End Patch
```
