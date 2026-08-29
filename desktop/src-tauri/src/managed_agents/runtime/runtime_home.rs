//! Provider CLI discovery isolation for Desktop-managed agents.
//!
//! Managed agents run in the Buzz nest, but provider CLIs normally discover
//! skills, plugins, and MCP servers from the human user's home directory. For
//! Codex that made a Buzz agent inherit unrelated host tools such as cmux and
//! Playwright. Point Codex at the nest's existing `.codex` directory instead,
//! while linking only its authentication file so login still works.

use std::path::{Path, PathBuf};

use super::super::KnownAcpRuntime;

const CODEX_SKILL_DIR: &str = ".codex/skills";
const CODEX_HOME_DIR: &str = ".codex";
const CODEX_HOME_ENV: &str = "CODEX_HOME";
const CODEX_AUTH_FILE: &str = "auth.json";

pub(super) fn apply_isolated_runtime_home(
    command: &mut std::process::Command,
    runtime: Option<&KnownAcpRuntime>,
    nest_root: Option<&Path>,
) -> Result<(), String> {
    let (Some(runtime), Some(nest_root)) = (runtime, nest_root) else {
        return Ok(());
    };
    if runtime.skill_dir != Some(CODEX_SKILL_DIR) {
        return Ok(());
    }

    let isolated_home = nest_root.join(CODEX_HOME_DIR);
    std::fs::create_dir_all(&isolated_home)
        .map_err(|error| format!("create {}: {error}", isolated_home.display()))?;

    if let Some(source_home) = host_codex_home() {
        link_codex_auth(&source_home, &isolated_home)?;
    }

    command.env(CODEX_HOME_ENV, isolated_home);
    Ok(())
}

fn host_codex_home() -> Option<PathBuf> {
    std::env::var_os(CODEX_HOME_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(CODEX_HOME_DIR)))
}

fn link_codex_auth(source_home: &Path, isolated_home: &Path) -> Result<(), String> {
    if source_home == isolated_home {
        return Ok(());
    }

    let source = source_home.join(CODEX_AUTH_FILE);
    if !source.is_file() {
        return Ok(());
    }

    let target = isolated_home.join(CODEX_AUTH_FILE);
    if target.symlink_metadata().is_ok() {
        return Ok(());
    }

    link_auth_file(&source, &target)
}

#[cfg(unix)]
fn link_auth_file(source: &Path, target: &Path) -> Result<(), String> {
    std::os::unix::fs::symlink(source, target).map_err(|error| {
        format!(
            "link Codex authentication {} -> {}: {error}",
            target.display(),
            source.display()
        )
    })
}

#[cfg(windows)]
fn link_auth_file(source: &Path, target: &Path) -> Result<(), String> {
    // File symlinks require elevated privileges on some Windows installs.
    // A first-use copy keeps the isolated home usable there; a user can log in
    // directly against that home if the credential later changes.
    std::fs::copy(source, target).map(|_| ()).map_err(|error| {
        format!(
            "copy Codex authentication {} -> {}: {error}",
            source.display(),
            target.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_auth_is_available_without_host_config() {
        let temp = tempfile::tempdir().expect("tempdir");
        let source_home = temp.path().join("host-codex");
        let isolated_home = temp.path().join("nest/.codex");
        std::fs::create_dir_all(&source_home).expect("source home");
        std::fs::create_dir_all(&isolated_home).expect("isolated home");
        std::fs::write(source_home.join(CODEX_AUTH_FILE), "credential").expect("auth fixture");
        std::fs::write(source_home.join("config.toml"), "[mcp_servers.playwright]")
            .expect("config fixture");

        link_codex_auth(&source_home, &isolated_home).expect("link auth");

        assert_eq!(
            std::fs::read_to_string(isolated_home.join(CODEX_AUTH_FILE)).expect("read linked auth"),
            "credential"
        );
        assert!(!isolated_home.join("config.toml").exists());
    }

    #[test]
    fn existing_isolated_auth_is_never_overwritten() {
        let temp = tempfile::tempdir().expect("tempdir");
        let source_home = temp.path().join("host-codex");
        let isolated_home = temp.path().join("nest/.codex");
        std::fs::create_dir_all(&source_home).expect("source home");
        std::fs::create_dir_all(&isolated_home).expect("isolated home");
        std::fs::write(source_home.join(CODEX_AUTH_FILE), "host").expect("host auth");
        std::fs::write(isolated_home.join(CODEX_AUTH_FILE), "isolated").expect("isolated auth");

        link_codex_auth(&source_home, &isolated_home).expect("keep isolated auth");

        assert_eq!(
            std::fs::read_to_string(isolated_home.join(CODEX_AUTH_FILE)).expect("read auth"),
            "isolated"
        );
    }
}
