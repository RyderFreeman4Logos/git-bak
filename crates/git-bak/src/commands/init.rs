use std::fs;
use std::path::{Path, PathBuf};

use git_bak_core::{Error, GitRepo, Result};

pub fn execute(path: Option<PathBuf>) -> Result<()> {
    let workspace = match path {
        Some(path) => path,
        None => std::env::current_dir()
            .map_err(|source| Error::Config(format!("failed to resolve current directory: {source}")))?,
    };

    fs::create_dir_all(&workspace).map_err(|source| {
        Error::Config(format!(
            "failed to create workspace directory {}: {source}",
            workspace.display()
        ))
    })?;

    let config_path = workspace.join("personaguard.toml");
    let config_content = default_config_contents(&workspace);
    fs::write(&config_path, config_content).map_err(|source| {
        Error::Config(format!(
            "failed to write config file {}: {source}",
            config_path.display()
        ))
    })?;

    let ignore_path = workspace.join(".gitignore");
    let ignore_content = default_gitignore_contents();
    fs::write(&ignore_path, ignore_content).map_err(|source| {
        Error::Config(format!(
            "failed to write .gitignore file {}: {source}",
            ignore_path.display()
        ))
    })?;

    ensure_default_watch_files(&workspace)?;
    let _repo = GitRepo::init(&workspace)?;
    println!("initialized git-bak workspace at {}", workspace.display());
    Ok(())
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

fn default_config_contents(workspace: &Path) -> String {
    format!(
        "workspace = \"{}\"\nwatch = [\"SOUL.md\", \"AGENTS.md\", \"USER.md\", \"IDENTITY.md\", \"TOOLS.md\", \"MEMORY.md\", \"memory/*.md\"]\ndebounce_ms = 1500\npush_interval_sec = 600\nmode = \"watcher\"\n",
        workspace.display()
    )
}

fn default_gitignore_contents() -> &'static str {
    "target/\n*.swp\n.env\n.env.*\n*.pem\n*.key\n.csa/\n.weave/\n"
}
