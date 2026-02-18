use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use git_bak_core::{Error, GitRepo, Result};

pub fn execute(path: Option<PathBuf>) -> Result<()> {
    let workspace = resolve_workspace_path(path)?;

    ensure_default_config_file(&workspace)?;
    ensure_default_gitignore_entries(&workspace)?;

    ensure_default_watch_files(&workspace)?;
    let _repo = GitRepo::init(&workspace)?;
    println!("initialized git-bak workspace at {}", workspace.display());
    Ok(())
}

fn resolve_workspace_path(path: Option<PathBuf>) -> Result<PathBuf> {
    let raw_workspace = match path {
        Some(path) => path,
        None => std::env::current_dir().map_err(|source| {
            Error::Config(format!("failed to resolve current directory: {source}"))
        })?,
    };

    fs::create_dir_all(&raw_workspace).map_err(|source| {
        Error::Config(format!(
            "failed to create workspace directory {}: {source}",
            raw_workspace.display()
        ))
    })?;

    fs::canonicalize(&raw_workspace).map_err(|source| {
        Error::Config(format!(
            "failed to normalize workspace path {}: {source}",
            raw_workspace.display()
        ))
    })
}

fn ensure_default_watch_files(workspace: &Path) -> Result<()> {
    const FILES: &[&str] = &[
        "SOUL.md",
        "AGENTS.md",
        "USER.md",
        "IDENTITY.md",
        "TOOLS.md",
        "MEMORY.md",
    ];

    let memory_dir = workspace.join("memory");
    fs::create_dir_all(&memory_dir).map_err(|source| {
        Error::Config(format!(
            "failed to create memory directory {}: {source}",
            memory_dir.display()
        ))
    })?;

    for relative in FILES {
        let file_path = workspace.join(relative);
        if file_path.exists() {
            continue;
        }
        fs::write(&file_path, "").map_err(|source| {
            Error::Config(format!(
                "failed to create default file {}: {source}",
                file_path.display()
            ))
        })?;
    }

    let memory_index = memory_dir.join("index.md");
    if !memory_index.exists() {
        fs::write(&memory_index, "").map_err(|source| {
            Error::Config(format!(
                "failed to create default memory file {}: {source}",
                memory_index.display()
            ))
        })?;
    }

    Ok(())
}

fn ensure_default_config_file(workspace: &Path) -> Result<()> {
    let config_path = workspace.join("personaguard.toml");
    if config_path.exists() {
        return Ok(());
    }

    let config_content = default_config_contents(workspace);
    fs::write(&config_path, config_content).map_err(|source| {
        Error::Config(format!(
            "failed to write config file {}: {source}",
            config_path.display()
        ))
    })?;
    Ok(())
}

fn ensure_default_gitignore_entries(workspace: &Path) -> Result<()> {
    let ignore_path = workspace.join(".gitignore");
    let mut content = if ignore_path.exists() {
        fs::read_to_string(&ignore_path).map_err(|source| {
            Error::Config(format!(
                "failed to read .gitignore file {}: {source}",
                ignore_path.display()
            ))
        })?
    } else {
        String::new()
    };

    let mut existing = content
        .lines()
        .map(|line| line.trim_end().to_owned())
        .collect::<BTreeSet<String>>();
    let mut changed = false;
    for entry in default_gitignore_entries() {
        if existing.contains(*entry) {
            continue;
        }
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(entry);
        content.push('\n');
        existing.insert((*entry).to_owned());
        changed = true;
    }

    if changed || !ignore_path.exists() {
        fs::write(&ignore_path, content).map_err(|source| {
            Error::Config(format!(
                "failed to write .gitignore file {}: {source}",
                ignore_path.display()
            ))
        })?;
    }

    Ok(())
}

fn default_config_contents(workspace: &Path) -> String {
    let workspace_raw = workspace.to_string_lossy().into_owned();
    let workspace_escaped = escape_toml_basic_string(&workspace_raw);
    format!(
        "workspace = \"{}\"\nwatch = [\"SOUL.md\", \"AGENTS.md\", \"USER.md\", \"IDENTITY.md\", \"TOOLS.md\", \"MEMORY.md\", \"memory/*.md\"]\ndebounce_ms = 1500\npush_interval_sec = 600\nmode = \"watcher\"\n",
        workspace_escaped
    )
}

fn escape_toml_basic_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn default_gitignore_entries() -> &'static [&'static str] {
    &[
        "target/", "*.swp", ".env", ".env.*", "*.pem", "*.key", ".csa/", ".weave/",
    ]
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        ensure_default_config_file, ensure_default_gitignore_entries, resolve_workspace_path,
    };

    #[test]
    fn test_ensure_default_config_file_keeps_existing_content() {
        let workspace = unique_test_workspace("config-keep");
        let config_path = workspace.join("personaguard.toml");
        let original = "workspace = \"/tmp/existing\"\nmode = \"hook\"\n";
        let write_result = fs::write(&config_path, original);
        assert!(write_result.is_ok());

        let result = ensure_default_config_file(&workspace);
        assert!(result.is_ok());

        let current = fs::read_to_string(&config_path);
        assert!(current.is_ok());
        assert_eq!(current.unwrap_or_default(), original);

        cleanup_workspace(workspace);
    }

    #[test]
    fn test_ensure_default_gitignore_entries_appends_missing_entries() {
        let workspace = unique_test_workspace("gitignore-append");
        let ignore_path = workspace.join(".gitignore");
        let initial = "custom/\n.env\n";
        let write_result = fs::write(&ignore_path, initial);
        assert!(write_result.is_ok());

        let result = ensure_default_gitignore_entries(&workspace);
        assert!(result.is_ok());

        let content = fs::read_to_string(&ignore_path).unwrap_or_default();
        assert!(content.contains("custom/\n"));
        assert!(content.contains("target/\n"));
        assert!(content.contains(".weave/\n"));

        let env_count = content.lines().filter(|line| *line == ".env").count();
        assert_eq!(env_count, 1);

        cleanup_workspace(workspace);
    }

    #[test]
    fn test_resolve_workspace_path_returns_absolute_path() {
        let workspace = unique_test_workspace("resolve-abs");
        let resolved = resolve_workspace_path(Some(workspace.clone()));
        assert!(resolved.is_ok());
        let resolved = resolved.unwrap_or_default();
        assert!(resolved.is_absolute());
        assert_eq!(resolved, workspace);

        cleanup_workspace(workspace);
    }

    fn unique_test_workspace(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let path = std::env::temp_dir().join(format!(
            "git-bak-init-{prefix}-{}-{nanos}",
            std::process::id()
        ));
        let create_result = fs::create_dir_all(&path);
        assert!(create_result.is_ok());
        path
    }

    fn cleanup_workspace(path: PathBuf) {
        let _ = fs::remove_dir_all(path);
    }
}
