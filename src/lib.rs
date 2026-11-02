pub mod config;

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSpec {
    pub name: String,
    pub pinned: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    UpToDate,
    Outdated,
    PinnedOutdated,
    Ahead,
    Missing,
    Unknown,
}

/// Parses a Pi package source such as `npm:@scope/name@1.2.3`; non-npm sources return `None`.
pub fn parse_source(source: &str) -> Option<PackageSpec> {
    let spec = source.strip_prefix("npm:")?;
    // A scoped name starts with '@', so the version separator is the next '@'.
    let search_from = usize::from(spec.starts_with('@'));
    let (name, pinned) = match spec[search_from..].find('@') {
        Some(i) => (
            &spec[..search_from + i],
            Some(spec[search_from + i + 1..].to_string()),
        ),
        None => (spec, None),
    };
    if name.is_empty() {
        return None;
    }
    Some(PackageSpec {
        name: name.to_string(),
        pinned,
    })
}

/// Lists the npm packages declared in Pi's `settings.json`.
pub fn packages_from_settings(settings: &Value) -> Vec<PackageSpec> {
    let Some(entries) = settings.get("packages").and_then(Value::as_array) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| entry.as_str().or_else(|| entry.get("source")?.as_str()))
        .filter_map(parse_source)
        .collect()
}

pub fn classify(installed: Option<&str>, latest: Option<&str>, pinned: bool) -> Status {
    let Some(installed) = installed else {
        return Status::Missing;
    };
    let parse = |v: &str| semver::Version::parse(v.trim_start_matches('v')).ok();
    let (Some(installed), Some(latest)) = (parse(installed), latest.and_then(parse)) else {
        return Status::Unknown;
    };
    match installed.cmp(&latest) {
        std::cmp::Ordering::Equal => Status::UpToDate,
        std::cmp::Ordering::Greater => Status::Ahead,
        std::cmp::Ordering::Less if pinned => Status::PinnedOutdated,
        std::cmp::Ordering::Less => Status::Outdated,
    }
}

/// Finds the first `x.y.z` token in command output or a release tag.
pub fn extract_version(text: &str) -> Option<String> {
    text.split_whitespace()
        .map(|token| token.trim_start_matches('v'))
        .find(|token| semver::Version::parse(token).is_ok())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spec(name: &str, pinned: Option<&str>) -> PackageSpec {
        PackageSpec {
            name: name.into(),
            pinned: pinned.map(Into::into),
        }
    }

    #[test]
    fn parses_npm_sources() {
        assert_eq!(parse_source("npm:gentle-pi"), Some(spec("gentle-pi", None)));
        assert_eq!(
            parse_source("npm:gentle-engram@0.4.2"),
            Some(spec("gentle-engram", Some("0.4.2")))
        );
        assert_eq!(
            parse_source("npm:@scope/pkg"),
            Some(spec("@scope/pkg", None))
        );
        assert_eq!(
            parse_source("npm:@scope/pkg@1.0.0"),
            Some(spec("@scope/pkg", Some("1.0.0")))
        );
    }

    #[test]
    fn ignores_non_npm_sources() {
        assert_eq!(parse_source("git:github.com/x/y"), None);
        assert_eq!(parse_source("./local/ext"), None);
        assert_eq!(parse_source("npm:"), None);
    }

    #[test]
    fn reads_string_and_object_package_entries() {
        let settings = json!({
            "packages": [
                { "source": "npm:pi-web-access@0.30.0", "extensions": [] },
                "npm:gentle-pi",
                "git:github.com/x/y"
            ]
        });
        assert_eq!(
            packages_from_settings(&settings),
            vec![
                spec("pi-web-access", Some("0.30.0")),
                spec("gentle-pi", None)
            ]
        );
        assert!(packages_from_settings(&json!({})).is_empty());
    }

    #[test]
    fn classifies_versions() {
        assert_eq!(
            classify(Some("1.0.0"), Some("1.0.0"), false),
            Status::UpToDate
        );
        assert_eq!(
            classify(Some("1.0.0"), Some("1.2.0"), false),
            Status::Outdated
        );
        assert_eq!(
            classify(Some("1.0.0"), Some("1.2.0"), true),
            Status::PinnedOutdated
        );
        assert_eq!(
            classify(Some("0.4.2"), Some("0.4.2"), true),
            Status::UpToDate
        );
        assert_eq!(classify(Some("2.0.0"), Some("1.9.0"), false), Status::Ahead);
        assert_eq!(classify(None, Some("1.0.0"), false), Status::Missing);
        assert_eq!(classify(Some("1.0.0"), None, false), Status::Unknown);
    }

    #[test]
    fn extracts_versions_from_output_and_tags() {
        assert_eq!(extract_version("engram 3.6.0\n").as_deref(), Some("3.6.0"));
        assert_eq!(extract_version("v4.3.1").as_deref(), Some("4.3.1"));
        assert_eq!(extract_version("1.5.0").as_deref(), Some("1.5.0"));
        assert_eq!(extract_version("no version here"), None);
    }
}
