use std::path::Path;

#[derive(Debug, Default, Clone, Copy)]
pub struct PathFilter;

impl PathFilter {
    pub fn matches_allowlist(path: &Path, patterns: &[String]) -> bool {
        let normalized_path = normalize_path(path);
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();

        patterns
            .iter()
            .any(|pattern| matches_glob(pattern, &normalized_path) || matches_glob(pattern, file_name))
    }

    pub fn matches_denylist(path: &Path) -> bool {
        let normalized_path = normalize_path(path);
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();

        if file_name == ".env" || file_name.starts_with(".env.") {
            return true;
        }
        if matches_glob("*.pem", file_name) || matches_glob("*.key", file_name) {
            return true;
        }
        normalized_path.starts_with("secrets/") || normalized_path.contains("/secrets/")
    }
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn matches_glob(pattern: &str, text: &str) -> bool {
    let pattern_chars: Vec<char> = pattern.chars().collect();
    let text_chars: Vec<char> = text.chars().collect();

    let mut dp = vec![vec![false; text_chars.len() + 1]; pattern_chars.len() + 1];
    dp[0][0] = true;

    for i in 1..=pattern_chars.len() {
        if pattern_chars[i - 1] == '*' {
            dp[i][0] = dp[i - 1][0];
        }
    }

    for i in 1..=pattern_chars.len() {
        for j in 1..=text_chars.len() {
            let pattern_char = pattern_chars[i - 1];
            let text_char = text_chars[j - 1];

            dp[i][j] = match pattern_char {
                '*' => dp[i - 1][j] || dp[i][j - 1],
                '?' => dp[i - 1][j - 1],
                _ => pattern_char == text_char && dp[i - 1][j - 1],
            };
        }
    }

    dp[pattern_chars.len()][text_chars.len()]
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::PathFilter;

    #[test]
    fn test_matches_allowlist_supports_file_and_glob_patterns() {
        let patterns = vec![
            "SOUL.md".to_owned(),
            "memory/*.md".to_owned(),
            "AGENTS.md".to_owned(),
        ];

        assert!(PathFilter::matches_allowlist(Path::new("SOUL.md"), &patterns));
        assert!(PathFilter::matches_allowlist(
            Path::new("memory/session.md"),
            &patterns
        ));
        assert!(!PathFilter::matches_allowlist(
            Path::new("notes.txt"),
            &patterns
        ));
    }

    #[test]
    fn test_matches_denylist_blocks_secret_patterns() {
        assert!(PathFilter::matches_denylist(Path::new(".env")));
        assert!(PathFilter::matches_denylist(Path::new(".env.local")));
        assert!(PathFilter::matches_denylist(Path::new("keys/private.key")));
        assert!(PathFilter::matches_denylist(Path::new("tls/cert.pem")));
        assert!(PathFilter::matches_denylist(Path::new("secrets/token.txt")));
        assert!(!PathFilter::matches_denylist(Path::new("memory/notes.md")));
    }
}
