use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, SystemTime};

use pi_check::config::{self, Drift};
use pi_check::{Status, classify, extract_version, packages_from_settings};
use serde_json::Value;

const PI_PACKAGE: &str = "@earendil-works/pi-coding-agent";
const GENTLE_AI_REPO: &str = "Gentleman-Programming/gentle-ai";
const ENGRAM_REPO: &str = "Gentleman-Programming/engram";
const ENGRAM_DEFAULT_PORT: u16 = 7437;
const PACKAGE_UPDATE: &str = "pi update --extensions";

const HELP: &str = "\
pi-check: compare the local Pi setup against the latest published versions

Usage: pi-check [update] [--agent-dir <path>] [--engram-port <port>]
       pi-check config <status|apply|capture> [paths...] [--repo <dir>] [--home <dir>]

Checks Pi, every npm package in <agent-dir>/settings.json, the gentle-ai
binary bundled with gentle-pi, the Engram CLI and the running Engram server.
Latest versions are always fetched live from npm and GitHub releases.
Set GITHUB_TOKEN to avoid GitHub's anonymous rate limit.

Commands:
  update                Run `pi update --all --no-approve` and, when Homebrew
                        owns Engram, `brew upgrade --formula engram`; then check again.
                        Restart Pi afterwards to load updated packages.
  config status         Compare every file tracked in <repo>/home with <home>.
                        Exits 1 when any file differs or is missing.
  config apply          Copy drifted files from <repo>/home into <home>, backing up
                        overwritten files to <home>/.pi/agent/backups/pi-config-<ts>.
                        Restart Pi afterwards to load the applied config.
  config capture [paths...]
                        Copy tracked files (plus the given files under <home>)
                        from <home> back into <repo>/home.

Options:
  --agent-dir <path>    Pi agent directory (default: ~/.pi/agent)
  --engram-port <port>  Engram HTTP port (default: 7437)
  --repo <dir>          pi_config repository (default: where pi-check was built from)
  --home <dir>          Home directory the config mirrors (default: $HOME)
  -h, --help            Show this help

Exit codes: 0 everything current (or deliberately pinned), 1 needs attention, 2 usage error.";

struct Row {
    name: String,
    installed: Option<String>,
    latest: Option<String>,
    status: Status,
    hint: String,
}

enum Mode {
    Check,
    Update,
    Config(ConfigAction),
}

enum ConfigAction {
    Status,
    Apply,
    Capture(Vec<PathBuf>),
}

struct Options {
    mode: Mode,
    agent_dir: PathBuf,
    engram_port: u16,
    repo: PathBuf,
    home: PathBuf,
}

fn main() -> ExitCode {
    let options = match parse_args(std::env::args().skip(1)) {
        Ok(Some(options)) => options,
        Ok(None) => {
            println!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("pi-check: {message}\n\n{HELP}");
            return ExitCode::from(2);
        }
    };

    if let Mode::Config(action) = &options.mode {
        return match run_config(action, &options.repo.join("home"), &options.home) {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::from(1),
            Err(error) => {
                eprintln!("pi-check: {error}");
                ExitCode::from(1)
            }
        };
    }

    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .http_status_as_error(false)
        .build()
        .new_agent();

    let updated = match options.mode {
        Mode::Check => true,
        Mode::Update => run_update(),
        Mode::Config(_) => unreachable!("config runs before the check"),
    };

    let rows = collect_rows(&agent, &options);
    print_rows(&rows);

    let needs_attention = !updated
        || rows.iter().any(|row| {
            matches!(
                row.status,
                Status::Outdated | Status::Missing | Status::Unknown
            )
        });
    if needs_attention {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Option<Options>, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let mut options = Options {
        mode: Mode::Check,
        agent_dir: home.join(".pi/agent"),
        engram_port: ENGRAM_DEFAULT_PORT,
        repo: PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        home,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "update" if !matches!(options.mode, Mode::Config(_)) => options.mode = Mode::Update,
            "config" if matches!(options.mode, Mode::Check) => {
                let action = match args.next().as_deref() {
                    Some("status") => ConfigAction::Status,
                    Some("apply") => ConfigAction::Apply,
                    Some("capture") => ConfigAction::Capture(Vec::new()),
                    Some(other) => return Err(format!("unknown config action: {other}")),
                    None => return Err("config needs an action: status, apply or capture".into()),
                };
                options.mode = Mode::Config(action);
            }
            "--repo" => options.repo = args.next().ok_or("--repo needs a directory")?.into(),
            "--home" => options.home = args.next().ok_or("--home needs a directory")?.into(),
            "--agent-dir" => {
                options.agent_dir = args.next().ok_or("--agent-dir needs a path")?.into();
            }
            "--engram-port" => {
                let value = args.next().ok_or("--engram-port needs a port")?;
                options.engram_port = value
                    .parse()
                    .map_err(|_| format!("invalid port: {value}"))?;
            }
            other => match &mut options.mode {
                Mode::Config(ConfigAction::Capture(paths)) if !other.starts_with('-') => {
                    paths.push(other.into());
                }
                _ => return Err(format!("unknown argument: {other}")),
            },
        }
    }
    Ok(Some(options))
}

/// Runs a `config` action; returns whether the result needs no attention.
fn run_config(action: &ConfigAction, repo_home: &Path, home: &Path) -> io::Result<bool> {
    match action {
        ConfigAction::Status => {
            let mut in_sync = true;
            for (rel, drift) in config::status(repo_home, home)? {
                let label = match drift {
                    Drift::Same => "same",
                    Drift::Differs => "differs",
                    Drift::MissingAtHome => "missing",
                };
                in_sync &= drift == Drift::Same;
                println!("{label:<7}  {}", rel.display());
            }
            Ok(in_sync)
        }
        ConfigAction::Apply => {
            let seconds = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs());
            let backup_dir = home.join(format!(".pi/agent/backups/pi-config-{seconds}"));
            let written = config::apply(repo_home, home, &backup_dir)?;
            if written.is_empty() {
                println!("config already applied");
                return Ok(true);
            }
            for rel in &written {
                println!("wrote {}", home.join(rel).display());
            }
            // Only overwritten files are backed up, so the directory may not exist.
            if backup_dir.is_dir() {
                println!("backups in {}", backup_dir.display());
            }
            println!("Restart Pi to load the applied config.");
            Ok(true)
        }
        ConfigAction::Capture(paths) => {
            let extra = paths
                .iter()
                .map(|path| under_home(path, home))
                .collect::<io::Result<Vec<_>>>()?;
            let written = config::capture(repo_home, home, &extra)?;
            if written.is_empty() {
                println!("config already captured");
            }
            for rel in &written {
                println!("captured {}", repo_home.join(rel).display());
            }
            Ok(true)
        }
    }
}

/// Makes `path` absolute, spelled under `home` even when one of them goes through a symlink
/// (macOS `/var` vs `/private/var`), because `config::capture` compares paths literally.
fn under_home(path: &Path, home: &Path) -> io::Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    if absolute.starts_with(home) {
        return Ok(absolute);
    }
    let (Ok(real), Ok(real_home)) = (absolute.canonicalize(), home.canonicalize()) else {
        return Ok(absolute);
    };
    Ok(real
        .strip_prefix(&real_home)
        .map_or(absolute, |rel| home.join(rel)))
}

fn config_row(options: &Options) -> Row {
    let (status, hint) = match config::status(&options.repo.join("home"), &options.home) {
        Ok(files) => match files
            .iter()
            .filter(|(_, drift)| *drift != Drift::Same)
            .count()
        {
            0 => (Status::UpToDate, String::new()),
            drifted => (
                Status::Outdated,
                format!("{drifted} drifted; run `pi-check config status`"),
            ),
        },
        Err(error) => (Status::Unknown, format!("could not read config: {error}")),
    };
    Row {
        name: "config (pi_config)".into(),
        installed: None,
        latest: None,
        status,
        hint,
    }
}

/// Runs every update step even when an earlier one fails; returns whether all succeeded.
fn run_update() -> bool {
    let mut ok = run_step("pi", &["update", "--all", "--no-approve"]);
    let brew_owns_engram = Command::new("brew")
        .args(["list", "--formula", "--versions", "engram"])
        .output()
        .is_ok_and(|output| output.status.success());
    if brew_owns_engram {
        ok &= run_step("brew", &["upgrade", "--formula", "engram"]);
    } else {
        println!("==> skipping Engram: not installed with Homebrew");
    }
    println!();
    ok
}

fn run_step(program: &str, args: &[&str]) -> bool {
    println!("==> {program} {}", args.join(" "));
    match Command::new(program).args(args).status() {
        Ok(status) if status.success() => true,
        Ok(status) => {
            eprintln!("pi-check: `{program}` failed with {status}");
            false
        }
        Err(error) => {
            eprintln!("pi-check: could not run `{program}`: {error}");
            false
        }
    }
}

fn collect_rows(agent: &ureq::Agent, options: &Options) -> Vec<Row> {
    let settings_path = options.agent_dir.join("settings.json");
    let settings: Option<Value> = std::fs::read_to_string(&settings_path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok());
    let settings_row = settings.is_none().then(|| Row {
        name: "settings.json".into(),
        installed: None,
        latest: None,
        status: Status::Missing,
        hint: format!("unreadable at {}", settings_path.display()),
    });
    let settings = settings.unwrap_or(Value::Null);
    let node_modules = options.agent_dir.join("npm/node_modules");

    // Every row waits on the network, so each check runs on its own thread.
    std::thread::scope(|scope| {
        let mut handles = vec![scope.spawn(|| pi_row(agent))];
        for spec in packages_from_settings(&settings) {
            let node_modules = &node_modules;
            handles.push(scope.spawn(move || package_row(agent, node_modules, spec)));
        }
        handles.push(scope.spawn(|| gentle_ai_row(agent, &node_modules)));
        handles.push(scope.spawn(|| engram_rows(agent, options.engram_port)));
        let mut rows: Vec<Row> = settings_row.into_iter().collect();
        for handle in handles {
            rows.extend(handle.join().unwrap_or_default());
        }
        rows.push(config_row(options));
        rows
    })
}

fn pi_row(agent: &ureq::Agent) -> Vec<Row> {
    let installed = command_version("pi", &["--version"]);
    let latest = npm_latest(agent, PI_PACKAGE);
    let status = classify(installed.as_deref(), latest.as_deref(), false);
    vec![Row {
        name: "pi".into(),
        installed,
        latest,
        status,
        hint: hint(status, "pi update pi"),
    }]
}

fn package_row(agent: &ureq::Agent, node_modules: &Path, spec: pi_check::PackageSpec) -> Vec<Row> {
    let installed = read_package_version(&node_modules.join(&spec.name).join("package.json"));
    let latest = npm_latest(agent, &spec.name);
    let status = classify(
        installed.as_deref(),
        latest.as_deref(),
        spec.pinned.is_some(),
    );
    let hint = match (&spec.pinned, status) {
        (Some(pin), Status::PinnedOutdated) => format!("pinned @{pin} in settings.json"),
        (Some(pin), _) => format!("pinned @{pin}"),
        (None, _) => hint(status, PACKAGE_UPDATE),
    };
    vec![Row {
        name: spec.name,
        installed,
        latest,
        status,
        hint,
    }]
}

fn gentle_ai_row(agent: &ureq::Agent, node_modules: &Path) -> Vec<Row> {
    let binary = std::fs::read_dir(node_modules.join("gentle-pi/.gentle-ai"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path().join("gentle-ai"))
        .find(|path| path.is_file());
    let installed = binary.and_then(|path| command_version(&path.to_string_lossy(), &["version"]));
    let latest = github_latest(agent, GENTLE_AI_REPO);
    let status = classify(installed.as_deref(), latest.as_deref(), false);
    let hint = match status {
        Status::Outdated => "bundled with gentle-pi; wait for a gentle-pi release".into(),
        _ => hint(status, ""),
    };
    vec![Row {
        name: "gentle-ai (bundled)".into(),
        installed,
        latest,
        status,
        hint,
    }]
}

fn engram_rows(agent: &ureq::Agent, port: u16) -> Vec<Row> {
    let cli = command_version("engram", &["version"]);
    let latest = github_latest(agent, ENGRAM_REPO);
    let cli_status = classify(cli.as_deref(), latest.as_deref(), false);

    let server = get_json(agent, &format!("http://127.0.0.1:{port}/health"))
        .filter(|health| health.get("status").and_then(Value::as_str) == Some("ok"))
        .and_then(|health| health.get("version")?.as_str().and_then(extract_version));
    let server_status = classify(server.as_deref(), cli.as_deref(), false);
    let server_hint = match server_status {
        Status::Missing => format!("not reachable on :{port}; start `engram serve`"),
        Status::Outdated => "older than the CLI; restart the Engram server".into(),
        _ => String::new(),
    };

    vec![
        Row {
            name: "engram".into(),
            installed: cli.clone(),
            latest,
            status: cli_status,
            hint: hint(cli_status, "brew upgrade --formula engram"),
        },
        Row {
            name: "engram server".into(),
            installed: server,
            latest: cli,
            status: server_status,
            hint: server_hint,
        },
    ]
}

fn hint(status: Status, update_command: &str) -> String {
    match status {
        Status::Outdated if !update_command.is_empty() => format!("run `{update_command}`"),
        Status::Missing => "not installed".into(),
        Status::Unknown => "could not determine a version".into(),
        _ => String::new(),
    }
}

fn command_version(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    extract_version(&String::from_utf8_lossy(&output.stdout))
}

fn read_package_version(path: &Path) -> Option<String> {
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    manifest.get("version")?.as_str().map(str::to_string)
}

fn npm_latest(agent: &ureq::Agent, name: &str) -> Option<String> {
    let encoded = name.replace('/', "%2f");
    let manifest = get_json(
        agent,
        &format!("https://registry.npmjs.org/{encoded}/latest"),
    )?;
    manifest.get("version")?.as_str().map(str::to_string)
}

fn github_latest(agent: &ureq::Agent, repo: &str) -> Option<String> {
    let mut request = agent
        .get(&format!(
            "https://api.github.com/repos/{repo}/releases/latest"
        ))
        .header("Accept", "application/vnd.github+json");
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        request = request.header("Authorization", &format!("Bearer {token}"));
    }
    let mut response = request.call().ok()?;
    if !response.status().is_success() {
        return None;
    }
    let release: Value = response.body_mut().read_json().ok()?;
    extract_version(release.get("tag_name")?.as_str()?)
}

fn get_json(agent: &ureq::Agent, url: &str) -> Option<Value> {
    let mut response = agent.get(url).call().ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.body_mut().read_json().ok()
}

fn print_rows(rows: &[Row]) {
    let color = std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    let width = rows.iter().map(|row| row.name.len()).max().unwrap_or(0);
    println!(
        "  {:<width$}  {:<10}  {:<10}  status",
        "component", "installed", "latest"
    );
    for row in rows {
        let (label, code) = match row.status {
            Status::UpToDate => ("ok", "32"),
            Status::Ahead => ("ahead", "36"),
            Status::Outdated => ("outdated", "33"),
            Status::PinnedOutdated => ("pinned", "35"),
            Status::Missing => ("missing", "31"),
            Status::Unknown => ("unknown", "31"),
        };
        let label = if color {
            format!("\x1b[{code}m{label}\x1b[0m")
        } else {
            label.to_string()
        };
        let dash = || "-".to_string();
        let hint = if row.hint.is_empty() {
            String::new()
        } else {
            format!("  {}", row.hint)
        };
        println!(
            "  {:<width$}  {:<10}  {:<10}  {label}{hint}",
            row.name,
            row.installed.clone().unwrap_or_else(dash),
            row.latest.clone().unwrap_or_else(dash),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> impl Iterator<Item = String> {
        list.iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn parses_update_subcommand() {
        let options = parse_args(args(&["update", "--engram-port", "9000"]))
            .unwrap()
            .unwrap();
        assert!(matches!(options.mode, Mode::Update));
        assert_eq!(options.engram_port, 9000);
        let options = parse_args(args(&[])).unwrap().unwrap();
        assert!(matches!(options.mode, Mode::Check));
        assert!(parse_args(args(&["upgrade"])).is_err());
    }

    #[test]
    fn parses_config_subcommand() {
        let options = parse_args(args(&["config", "status"])).unwrap().unwrap();
        assert!(matches!(options.mode, Mode::Config(ConfigAction::Status)));
        let options = parse_args(args(&["config", "capture", "a", "b", "--home", "/h"]))
            .unwrap()
            .unwrap();
        assert!(matches!(
            &options.mode,
            Mode::Config(ConfigAction::Capture(paths)) if *paths == [PathBuf::from("a"), PathBuf::from("b")]
        ));
        assert_eq!(options.home, PathBuf::from("/h"));
        let options = parse_args(args(&["config", "apply", "--repo", "/r"]))
            .unwrap()
            .unwrap();
        assert!(matches!(options.mode, Mode::Config(ConfigAction::Apply)));
        assert_eq!(options.repo, PathBuf::from("/r"));
    }

    #[test]
    fn rejects_missing_or_unknown_config_action() {
        assert!(parse_args(args(&["config"])).is_err());
        assert!(parse_args(args(&["config", "bogus"])).is_err());
        assert!(parse_args(args(&["config", "status", "extra"])).is_err());
    }

    #[test]
    fn resolves_capture_paths_through_a_symlinked_home() {
        let dir = tempfile::tempdir().unwrap();
        let real_home = dir.path().join("real");
        std::fs::create_dir_all(real_home.join(".pi")).unwrap();
        std::fs::write(real_home.join(".pi/a.json"), "{}").unwrap();
        let home = dir.path().join("home");
        std::os::unix::fs::symlink(&real_home, &home).unwrap();

        assert_eq!(
            under_home(&real_home.join(".pi/a.json"), &home).unwrap(),
            home.join(".pi/a.json")
        );
        assert_eq!(
            under_home(&home.join(".pi/a.json"), &home).unwrap(),
            home.join(".pi/a.json")
        );
        let outside = dir.path().join("outside");
        assert_eq!(under_home(&outside, &home).unwrap(), outside);
    }

    #[test]
    fn package_hint_names_the_extensions_update() {
        assert_eq!(
            hint(Status::Outdated, PACKAGE_UPDATE),
            "run `pi update --extensions`"
        );
    }
}
