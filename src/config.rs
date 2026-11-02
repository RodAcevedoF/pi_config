//! Tracked configuration: `<repo>/home/` mirrors `$HOME`, one file per tracked path.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

/// Keys that tools rewrite on their own; never tracked, and kept from `$HOME` on apply.
const VOLATILE_KEYS: &[(&str, &[&str])] = &[
    (
        ".pi/agent/settings.json",
        &["deviceId", "lastChangelogVersion"],
    ),
    (".pi/agent/claude-bridge.json", &["startupNoticeShown"]),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drift {
    Same,
    Differs,
    MissingAtHome,
}

/// Lists tracked files as paths relative to `repo_home`, sorted.
pub fn tracked_files(repo_home: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if repo_home.is_dir() {
        walk(repo_home, Path::new(""), &mut files)?;
    }
    files.sort();
    Ok(files)
}

fn walk(root: &Path, rel: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(root.join(rel))? {
        let entry = entry?;
        let child = rel.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            walk(root, &child, files)?;
        } else if entry.file_name() != ".DS_Store" {
            files.push(child);
        }
    }
    Ok(())
}

pub fn status(repo_home: &Path, home: &Path) -> io::Result<Vec<(PathBuf, Drift)>> {
    tracked_files(repo_home)?
        .into_iter()
        .map(|rel| {
            let target = home.join(&rel);
            let drift = if !target.is_file() {
                Drift::MissingAtHome
            } else if same(&rel, &fs::read(repo_home.join(&rel))?, &fs::read(&target)?) {
                Drift::Same
            } else {
                Drift::Differs
            };
            Ok((rel, drift))
        })
        .collect()
}

/// Copies every drifted file into `home`, backing up overwritten files under `backup_dir`.
pub fn apply(repo_home: &Path, home: &Path, backup_dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for (rel, drift) in status(repo_home, home)? {
        let source = repo_home.join(&rel);
        let target = home.join(&rel);
        match drift {
            Drift::Same => continue,
            Drift::MissingAtHome => {
                fs::create_dir_all(target.parent().unwrap_or(home))?;
                // fs::copy also carries the mode, e.g. the executable bit of a wrapper script.
                fs::copy(&source, &target)?;
            }
            Drift::Differs => {
                let current = fs::read(&target)?;
                let backup = backup_dir.join(&rel);
                fs::create_dir_all(backup.parent().unwrap_or(backup_dir))?;
                fs::write(&backup, &current)?;
                // fs::write keeps the target's existing mode (e.g. 0600 on private files).
                fs::write(&target, merge_volatile(&rel, &fs::read(&source)?, &current))?;
            }
        }
        written.push(rel);
    }
    Ok(written)
}

/// Copies tracked files (plus `extra` absolute paths under `home`) into the repo without volatile keys.
pub fn capture(repo_home: &Path, home: &Path, extra: &[PathBuf]) -> io::Result<Vec<PathBuf>> {
    let mut rels: BTreeSet<PathBuf> = tracked_files(repo_home)?.into_iter().collect();
    for path in extra {
        rels.insert(relative_to_home(path, home)?);
    }
    let mut written = Vec::new();
    for rel in rels {
        let source = home.join(&rel);
        if !source.is_file() {
            continue;
        }
        let content = strip_volatile(&rel, &fs::read(&source)?);
        let target = repo_home.join(&rel);
        if target.is_file() && fs::read(&target)? == content {
            continue;
        }
        let is_new = !target.exists();
        fs::create_dir_all(target.parent().unwrap_or(repo_home))?;
        fs::write(&target, &content)?;
        if is_new {
            let mode = fs::metadata(&source)?.permissions().mode();
            fs::set_permissions(&target, fs::Permissions::from_mode(mode))?;
        }
        written.push(rel);
    }
    Ok(written)
}

fn relative_to_home(path: &Path, home: &Path) -> io::Result<PathBuf> {
    let rel = path
        .strip_prefix(home)
        .ok()
        .filter(|rel| rel.components().all(|c| matches!(c, Component::Normal(_))))
        .filter(|rel| !rel.as_os_str().is_empty());
    match rel {
        Some(rel) if path.is_file() => Ok(rel.to_path_buf()),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not a file under {}", path.display(), home.display()),
        )),
    }
}

fn volatile_keys(rel: &Path) -> &'static [&'static str] {
    VOLATILE_KEYS
        .iter()
        .find(|(path, _)| Path::new(path) == rel)
        .map_or(&[], |(_, keys)| keys)
}

fn parse_object(bytes: &[u8]) -> Option<serde_json::Map<String, Value>> {
    match serde_json::from_slice(bytes).ok()? {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

fn without_volatile(rel: &Path, bytes: &[u8]) -> Option<serde_json::Map<String, Value>> {
    let keys = volatile_keys(rel);
    if keys.is_empty() {
        return None;
    }
    let mut map = parse_object(bytes)?;
    map.retain(|key, _| !keys.contains(&key.as_str()));
    Some(map)
}

fn same(rel: &Path, repo: &[u8], home: &[u8]) -> bool {
    match (without_volatile(rel, repo), without_volatile(rel, home)) {
        (Some(repo), Some(home)) => repo == home,
        _ => repo == home,
    }
}

fn strip_volatile(rel: &Path, bytes: &[u8]) -> Vec<u8> {
    match without_volatile(rel, bytes) {
        Some(map) => pretty(&map, "\n"),
        None => bytes.to_vec(),
    }
}

fn merge_volatile(rel: &Path, repo: &[u8], home: &[u8]) -> Vec<u8> {
    let (Some(mut merged), Some(current)) = (without_volatile(rel, repo), parse_object(home))
    else {
        return repo.to_vec();
    };
    for key in volatile_keys(rel) {
        if let Some(value) = current.get(*key) {
            merged.insert((*key).to_string(), value.clone());
        }
    }
    // Pi writes these files without a trailing newline.
    pretty(&merged, "")
}

fn pretty(map: &serde_json::Map<String, Value>, end: &str) -> Vec<u8> {
    let mut text = serde_json::to_string_pretty(map).unwrap_or_default();
    text.push_str(end);
    text.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        _dir: tempfile::TempDir,
        repo: PathBuf,
        home: PathBuf,
        backup: PathBuf,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo/home");
        let home = dir.path().join("home");
        let backup = dir.path().join("backup");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&home).unwrap();
        Fixture {
            _dir: dir,
            repo,
            home,
            backup,
        }
    }

    fn put(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn read(root: &Path, rel: &str) -> String {
        fs::read_to_string(root.join(rel)).unwrap()
    }

    const SETTINGS: &str = ".pi/agent/settings.json";

    #[test]
    fn reports_same_differs_and_missing_ignoring_volatile_keys() {
        let f = fixture();
        put(&f.repo, SETTINGS, r#"{"theme": "a"}"#);
        put(
            &f.home,
            SETTINGS,
            r#"{"deviceId": "x", "theme": "a", "lastChangelogVersion": "1.5.0"}"#,
        );
        put(&f.repo, ".pi/agent/AGENTS.md", "rules v2");
        put(&f.home, ".pi/agent/AGENTS.md", "rules v1");
        put(&f.repo, ".local/bin/pi", "#!/bin/sh");

        assert_eq!(
            status(&f.repo, &f.home).unwrap(),
            vec![
                (PathBuf::from(".local/bin/pi"), Drift::MissingAtHome),
                (PathBuf::from(".pi/agent/AGENTS.md"), Drift::Differs),
                (PathBuf::from(SETTINGS), Drift::Same),
            ]
        );
    }

    #[test]
    fn apply_writes_drifted_files_backs_up_and_keeps_volatile_keys() {
        let f = fixture();
        put(&f.repo, SETTINGS, r#"{"theme": "b"}"#);
        put(&f.home, SETTINGS, r#"{"deviceId": "x", "theme": "a"}"#);
        put(&f.repo, ".pi/agent/AGENTS.md", "same");
        put(&f.home, ".pi/agent/AGENTS.md", "same");
        put(&f.repo, ".local/bin/pi", "#!/bin/sh");
        fs::set_permissions(
            f.repo.join(".local/bin/pi"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();

        let written = apply(&f.repo, &f.home, &f.backup).unwrap();

        assert_eq!(
            written,
            vec![PathBuf::from(".local/bin/pi"), PathBuf::from(SETTINGS)]
        );
        let settings: serde_json::Value = serde_json::from_str(&read(&f.home, SETTINGS)).unwrap();
        assert_eq!(settings, serde_json::json!({"theme": "b", "deviceId": "x"}));
        assert_eq!(
            read(&f.backup, SETTINGS),
            r#"{"deviceId": "x", "theme": "a"}"#
        );
        assert!(!f.backup.join(".pi/agent/AGENTS.md").exists());
        assert!(!f.backup.join(".local/bin/pi").exists());
        let mode = fs::metadata(f.home.join(".local/bin/pi"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);
        assert!(
            status(&f.repo, &f.home)
                .unwrap()
                .iter()
                .all(|(_, d)| *d == Drift::Same)
        );
    }

    #[test]
    fn apply_keeps_existing_permissions() {
        let f = fixture();
        let rel = ".pi/gentle-ai/profiles.json";
        put(&f.repo, rel, "{\"a\": 2}");
        put(&f.home, rel, "{\"a\": 1}");
        fs::set_permissions(f.home.join(rel), fs::Permissions::from_mode(0o600)).unwrap();

        apply(&f.repo, &f.home, &f.backup).unwrap();

        assert_eq!(read(&f.home, rel), "{\"a\": 2}");
        let mode = fs::metadata(f.home.join(rel)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn capture_strips_volatile_keys_and_adds_extra_paths() {
        let f = fixture();
        put(&f.repo, SETTINGS, r#"{"theme": "old"}"#);
        put(
            &f.home,
            SETTINGS,
            r#"{"deviceId": "x", "theme": "new", "lastChangelogVersion": "1"}"#,
        );
        put(
            &f.home,
            ".pi/agent/claude-bridge.json",
            r#"{"startupNoticeShown": "d", "plan": "max"}"#,
        );
        put(&f.home, ".gitignore_global", "/odd/\n");

        let extra = [
            f.home.join(".pi/agent/claude-bridge.json"),
            f.home.join(".gitignore_global"),
        ];
        let written = capture(&f.repo, &f.home, &extra).unwrap();

        assert_eq!(
            written,
            vec![
                PathBuf::from(".gitignore_global"),
                PathBuf::from(".pi/agent/claude-bridge.json"),
                PathBuf::from(SETTINGS),
            ]
        );
        assert_eq!(read(&f.repo, SETTINGS), "{\n  \"theme\": \"new\"\n}\n");
        assert_eq!(
            read(&f.repo, ".pi/agent/claude-bridge.json"),
            "{\n  \"plan\": \"max\"\n}\n"
        );
        assert_eq!(read(&f.repo, ".gitignore_global"), "/odd/\n");
        assert!(capture(&f.repo, &f.home, &[]).unwrap().is_empty());
    }

    #[test]
    fn capture_rejects_paths_outside_home_without_writing() {
        let f = fixture();
        let outside = f.home.parent().unwrap().join("outside.txt");
        fs::write(&outside, "x").unwrap();

        let error = capture(&f.repo, &f.home, &[outside]).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(tracked_files(&f.repo).unwrap().is_empty());
    }
}
