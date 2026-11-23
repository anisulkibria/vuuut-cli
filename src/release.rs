use crate::Result;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub fn checked(command: &mut Command, description: &str) -> Result<()> {
    let status = command
        .status()
        .map_err(|error| format!("{description}: {error}"))?;
    if !status.success() {
        return Err(format!("{description} exited with {status}").into());
    }
    Ok(())
}

fn capture(command: &mut Command, description: &str) -> Result<String> {
    let output = command
        .output()
        .map_err(|error| format!("{description}: {error}"))?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr);
        return Err(format!("{description}: {}", message.trim()).into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

pub fn prerequisites() -> Result<()> {
    for tool in ["git", "rustc", "cargo"] {
        checked(Command::new(tool).arg("--version"), tool)?;
    }
    Ok(())
}

fn git(source: &Path) -> Command {
    let mut command = Command::new("git");
    command.current_dir(source);
    command
}

/// Convert an existing local path to an absolute path before passing it to Git.
pub fn repository(value: &str) -> Result<String> {
    let path = Path::new(value);
    if path.exists() {
        return Ok(fs::canonicalize(path)?.to_string_lossy().into_owned());
    }
    Ok(value.to_owned())
}

pub fn clone_into(state: &Path, repo: &str) -> Result<()> {
    let source = state.join("source");
    if source.is_dir() {
        let existing = capture(
            git(&source).args(["remote", "get-url", "origin"]),
            "read origin",
        )?;
        if existing != repo {
            return Err(
                "An unfinished install in this directory uses a different repository.".into(),
            );
        }
        return Ok(());
    }
    let staged = state.join("source.clone");
    if staged.exists() {
        fs::remove_dir_all(&staged)?;
    }
    checked(
        Command::new("git")
            .args(["clone", "--no-checkout", "--origin", "origin", "--", repo])
            .arg(&staged),
        "download Vuuut",
    )?;
    fs::rename(staged, source)?;
    Ok(())
}

pub fn refresh(source: &Path) -> Result<()> {
    checked(
        git(source).args(["fetch", "--prune", "--tags", "origin"]),
        "fetch Vuuut updates",
    )
}

pub fn commit(source: &Path, reference: Option<&str>) -> Result<String> {
    let candidates = match reference {
        Some(reference) => vec![
            format!("refs/remotes/origin/{reference}"),
            format!("refs/tags/{reference}"),
            reference.to_owned(),
        ],
        None => vec!["refs/remotes/origin/HEAD".to_owned()],
    };
    for candidate in candidates {
        let revision = format!("{candidate}^{{commit}}");
        if let Ok(value) = capture(
            git(source).args(["rev-parse", "--verify", "--end-of-options", &revision]),
            "resolve Vuuut revision",
        ) {
            if valid_commit(&value) {
                return Ok(value);
            }
        }
    }
    Err(format!(
        "Could not resolve {} in the Vuuut repository.",
        reference.unwrap_or("the default branch")
    )
    .into())
}

pub fn valid_commit(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn binary(state: &Path, revision: &str) -> PathBuf {
    state.join("releases").join(revision).join("vuuut")
}

pub fn build(state: &Path, revision: &str) -> Result<PathBuf> {
    let source = state.join("source");
    checked(
        git(&source).args(["checkout", "--detach", revision]),
        "check out Vuuut",
    )?;
    let installed = binary(state, revision);
    if installed.is_file() {
        return Ok(installed);
    }
    let metadata = capture(
        Command::new("cargo").current_dir(&source).args([
            "metadata",
            "--locked",
            "--no-deps",
            "--format-version",
            "1",
        ]),
        "read the Vuuut Cargo workspace",
    )?;
    let metadata: Value = serde_json::from_str(&metadata)?;
    let binaries = metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|package| package["targets"].as_array().into_iter().flatten())
        .filter(|target| {
            target["kind"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"))
        })
        .filter_map(|target| target["name"].as_str())
        .collect::<Vec<_>>();
    // Older revisions of the same repository built the server as ghost.
    let name = ["vuuut", "ghost"]
        .into_iter()
        .find(|name| binaries.contains(name))
        .ok_or("The repository has no Vuuut server binary target.")?;
    let target = state.join("build");
    checked(
        Command::new("cargo")
            .current_dir(&source)
            .env("CARGO_TARGET_DIR", &target)
            .args(["build", "--locked", "--release", "--bin", name]),
        "build Vuuut",
    )?;
    fs::create_dir_all(installed.parent().expect("release path has a parent"))?;
    let staged = installed.with_extension("next");
    fs::copy(target.join("release").join(name), &staged)?;
    fs::rename(staged, &installed)?;
    Ok(installed)
}

pub fn seed_content(state: &Path, root: &Path) -> Result<()> {
    let shipped = state.join("source/content");
    let content = root.join("content");
    fs::create_dir_all(content.join("data"))?;
    copy_missing(&shipped.join("themes"), &content.join("themes"))?;
    copy_missing(&shipped.join("routes.yaml"), &content.join("routes.yaml"))?;
    Ok(())
}

fn copy_missing(from: &Path, to: &Path) -> Result<()> {
    if from.is_dir() {
        fs::create_dir_all(to)?;
        for child in fs::read_dir(from)? {
            let child = child?;
            if !child.file_type()?.is_symlink() {
                copy_missing(&child.path(), &to.join(child.file_name()))?;
            }
        }
    } else if from.is_file() && !to.exists() {
        fs::copy(from, to)?;
    }
    Ok(())
}
