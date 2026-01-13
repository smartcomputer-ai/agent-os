use std::collections::BTreeMap;

use aos_sys::HttpPublishRule;

#[cfg(test)]
use aos_sys::WorkspaceRef;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedPath {
    pub path: String,
    pub segments: Vec<String>,
    pub had_trailing_slash: bool,
    pub query: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    MissingLeadingSlash,
    InvalidPercentEncoding,
    InvalidUtf8,
    InvalidSegment(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchError {
    RequestPath(PathError),
    RulePrefix { id: String, error: PathError },
}

#[derive(Debug, Clone, Copy)]
pub struct PrefixRule<'a, T> {
    pub id: &'a str,
    pub rule: &'a T,
    pub prefix: &'a NormalizedPath,
}

pub fn normalize_request_path(raw: &str) -> Result<NormalizedPath, PathError> {
    normalize_path(raw, true)
}

pub fn normalize_route_prefix(raw: &str) -> Result<NormalizedPath, PathError> {
    normalize_path(raw, true)
}

pub fn prefix_matches(prefix: &NormalizedPath, path: &NormalizedPath) -> bool {
    let prefix_len = prefix.segments.len();
    if prefix_len > path.segments.len() {
        return false;
    }
    prefix
        .segments
        .iter()
        .zip(path.segments.iter())
        .all(|(left, right)| left == right)
}

pub fn select_longest_prefix<'a, T>(
    path: &NormalizedPath,
    rules: impl IntoIterator<Item = PrefixRule<'a, T>>,
) -> Option<PrefixRule<'a, T>> {
    let mut best: Option<PrefixRule<'a, T>> = None;
    let mut best_len = 0usize;
    for candidate in rules {
        if !prefix_matches(candidate.prefix, path) {
            continue;
        }
        let len = candidate.prefix.segments.len();
        if best.is_none() || len > best_len {
            best = Some(candidate);
            best_len = len;
        }
    }
    best
}

#[derive(Debug, Clone)]
pub struct PublishMatch<'a> {
    pub rule_id: &'a str,
    pub rule: &'a HttpPublishRule,
    pub request: NormalizedPath,
    pub suffix_segments: Vec<String>,
    pub suffix: String,
}

pub fn match_publish_rule<'a>(
    rules: &'a BTreeMap<String, HttpPublishRule>,
    request_path: &str,
) -> Result<Option<PublishMatch<'a>>, MatchError> {
    let request = normalize_request_path(request_path).map_err(MatchError::RequestPath)?;
    let mut best: Option<PublishMatch<'a>> = None;
    let mut best_len = 0usize;
    for (id, rule) in rules {
        let prefix = normalize_route_prefix(&rule.route_prefix)
            .map_err(|error| MatchError::RulePrefix {
                id: id.clone(),
                error,
            })?;
        if !prefix_matches(&prefix, &request) {
            continue;
        }
        let len = prefix.segments.len();
        if best.is_none() || len > best_len {
            let suffix_segments = request.segments[len..].to_vec();
            let suffix = suffix_segments.join("/");
            best = Some(PublishMatch {
                rule_id: id.as_str(),
                rule,
                request: request.clone(),
                suffix_segments,
                suffix,
            });
            best_len = len;
        }
    }
    Ok(best)
}

pub fn join_workspace_path(base: Option<&str>, suffix: &str) -> String {
    let base = base.unwrap_or("").trim_matches('/');
    if base.is_empty() {
        return suffix.to_string();
    }
    if suffix.is_empty() {
        return base.to_string();
    }
    format!("{base}/{suffix}")
}

fn normalize_path(raw: &str, require_leading_slash: bool) -> Result<NormalizedPath, PathError> {
    let raw = raw.split('#').next().unwrap_or("");
    let (raw_path, query) = match raw.split_once('?') {
        Some((path, query)) => (path, Some(query.to_string())),
        None => (raw, None),
    };
    let raw_path = if raw_path.is_empty() { "/" } else { raw_path };
    let decoded = percent_decode(raw_path)?;
    if require_leading_slash && !decoded.starts_with('/') {
        return Err(PathError::MissingLeadingSlash);
    }
    let collapsed = collapse_slashes(&decoded);
    let had_trailing_slash = collapsed.ends_with('/');
    let normalized = if collapsed != "/" && had_trailing_slash {
        collapsed.trim_end_matches('/').to_string()
    } else {
        collapsed
    };
    let segments = if normalized == "/" {
        Vec::new()
    } else {
        normalized
            .trim_start_matches('/')
            .split('/')
            .map(|segment| {
                if is_valid_segment(segment) {
                    Ok(segment.to_string())
                } else {
                    Err(PathError::InvalidSegment(segment.to_string()))
                }
            })
            .collect::<Result<Vec<String>, PathError>>()?
    };
    Ok(NormalizedPath {
        path: normalized,
        segments,
        had_trailing_slash,
        query,
    })
}

fn percent_decode(input: &str) -> Result<String, PathError> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0usize;
    while idx < bytes.len() {
        let byte = bytes[idx];
        if byte == b'%' {
            if idx + 2 >= bytes.len() {
                return Err(PathError::InvalidPercentEncoding);
            }
            let hi = hex_value(bytes[idx + 1])?;
            let lo = hex_value(bytes[idx + 2])?;
            out.push((hi << 4) | lo);
            idx += 3;
        } else {
            out.push(byte);
            idx += 1;
        }
    }
    String::from_utf8(out).map_err(|_| PathError::InvalidUtf8)
}

fn hex_value(byte: u8) -> Result<u8, PathError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(PathError::InvalidPercentEncoding),
    }
}

fn collapse_slashes(input: &str) -> String {
    let mut out = String::new();
    let mut last_was_slash = false;
    for ch in input.chars() {
        if ch == '/' {
            if !last_was_slash {
                out.push('/');
                last_was_slash = true;
            }
        } else {
            out.push(ch);
            last_was_slash = false;
        }
    }
    if out.is_empty() {
        out.push('/');
    }
    out
}

fn is_valid_segment(segment: &str) -> bool {
    if segment.is_empty() {
        return false;
    }
    segment.chars().all(is_url_safe_char)
}

fn is_url_safe_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '~' | '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_query_and_fragment() {
        let normalized = normalize_request_path("/app/index.html?x=1#y").unwrap();
        assert_eq!(normalized.path, "/app/index.html");
        assert_eq!(normalized.query, Some("x=1".to_string()));
        assert_eq!(
            normalized.segments,
            vec!["app".to_string(), "index.html".to_string()]
        );
    }

    #[test]
    fn normalize_collapses_and_trims_slashes() {
        let normalized = normalize_request_path("//foo///bar/").unwrap();
        assert_eq!(normalized.path, "/foo/bar");
        assert!(normalized.had_trailing_slash);
        assert_eq!(
            normalized.segments,
            vec!["foo".to_string(), "bar".to_string()]
        );
    }

    #[test]
    fn normalize_rejects_invalid_percent_encoding() {
        let err = normalize_request_path("/foo/%2").unwrap_err();
        assert_eq!(err, PathError::InvalidPercentEncoding);
    }

    #[test]
    fn normalize_rejects_invalid_segment_chars() {
        let err = normalize_request_path("/foo/bar$").unwrap_err();
        assert_eq!(err, PathError::InvalidSegment("bar$".to_string()));
    }

    #[test]
    fn normalize_allows_root() {
        let normalized = normalize_request_path("/").unwrap();
        assert_eq!(normalized.path, "/");
        assert!(normalized.segments.is_empty());
    }

    #[test]
    fn normalize_route_prefix_requires_leading_slash() {
        let err = normalize_route_prefix("app").unwrap_err();
        assert_eq!(err, PathError::MissingLeadingSlash);
    }

    #[test]
    fn prefix_matches_by_segment() {
        let prefix = normalize_route_prefix("/app").unwrap();
        let exact = normalize_request_path("/app").unwrap();
        let child = normalize_request_path("/app/assets/logo.png").unwrap();
        let sibling = normalize_request_path("/apple").unwrap();
        assert!(prefix_matches(&prefix, &exact));
        assert!(prefix_matches(&prefix, &child));
        assert!(!prefix_matches(&prefix, &sibling));
    }

    #[test]
    fn select_longest_prefix_prefers_longer_match() {
        let root = normalize_route_prefix("/").unwrap();
        let app = normalize_route_prefix("/app").unwrap();
        let assets = normalize_route_prefix("/app/assets").unwrap();
        let path = normalize_request_path("/app/assets/logo.png").unwrap();
        let rules = vec![
            PrefixRule {
                id: "root",
                rule: &1,
                prefix: &root,
            },
            PrefixRule {
                id: "app",
                rule: &2,
                prefix: &app,
            },
            PrefixRule {
                id: "assets",
                rule: &3,
                prefix: &assets,
            },
        ];
        let matched = select_longest_prefix(&path, rules).expect("match");
        assert_eq!(matched.id, "assets");
        assert_eq!(*matched.rule, 3);
    }

    #[test]
    fn match_publish_rule_returns_suffix_and_trailing_slash() {
        let rules = publish_rules(vec![
            ("/", "root"),
            ("/app", "app"),
            ("/app/assets", "assets"),
        ]);
        let matched = match_publish_rule(&rules, "/app/assets/logo.png?x=1")
            .unwrap()
            .expect("match");
        assert_eq!(matched.rule_id, "assets");
        assert_eq!(
            matched.suffix_segments,
            vec!["logo.png".to_string()]
        );
        assert_eq!(matched.suffix, "logo.png");
        assert_eq!(matched.request.query, Some("x=1".to_string()));
    }

    #[test]
    fn match_publish_rule_rejects_invalid_prefix() {
        let mut rules = publish_rules(vec![("/", "root")]);
        let mut bad = HttpPublishRule {
            route_prefix: "app".to_string(),
            workspace: workspace_ref("shell"),
            default_doc: None,
            allow_dir_listing: false,
            cache: "etag".to_string(),
        };
        rules.insert("bad".to_string(), bad.clone());
        let err = match_publish_rule(&rules, "/app").unwrap_err();
        assert_eq!(
            err,
            MatchError::RulePrefix {
                id: "bad".to_string(),
                error: PathError::MissingLeadingSlash
            }
        );
        bad.route_prefix = "/ok".to_string();
        rules.insert("bad".to_string(), bad);
        let matched = match_publish_rule(&rules, "/ok/").unwrap().expect("match");
        assert_eq!(matched.rule_id, "bad");
        assert!(matched.request.had_trailing_slash);
    }

    #[test]
    fn join_workspace_path_handles_empty_components() {
        assert_eq!(join_workspace_path(None, "index.html"), "index.html");
        assert_eq!(join_workspace_path(Some("app"), ""), "app");
        assert_eq!(join_workspace_path(Some("app"), "index.html"), "app/index.html");
        assert_eq!(join_workspace_path(Some("/app/"), "assets/logo.png"), "app/assets/logo.png");
    }

    fn publish_rules(entries: Vec<(&str, &str)>) -> BTreeMap<String, HttpPublishRule> {
        let mut rules = BTreeMap::new();
        for (prefix, id) in entries {
            rules.insert(
                id.to_string(),
                HttpPublishRule {
                    route_prefix: prefix.to_string(),
                    workspace: workspace_ref("shell"),
                    default_doc: None,
                    allow_dir_listing: false,
                    cache: "etag".to_string(),
                },
            );
        }
        rules
    }

    fn workspace_ref(name: &str) -> WorkspaceRef {
        WorkspaceRef {
            workspace: name.to_string(),
            version: None,
            path: None,
        }
    }
}
