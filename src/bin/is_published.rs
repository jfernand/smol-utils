use clap::Parser;
use semver::Version;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

#[derive(Parser, Debug)]
#[command(
    name = "is-published",
    version,
    about = "Determine if the current folder is a published crate, and if so, whether it is up to date",
    long_about = None
)]
pub struct Args {
    /// Path to the crate or workspace directory
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Cargo registry to check against
    #[arg(short, long, default_value = "crates-io")]
    pub registry: String,

    /// Do not print output, only exit with status code
    #[arg(short, long)]
    pub quiet: bool,

    /// Fail if there are uncommitted git changes in the working tree
    #[arg(long)]
    pub strict: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PublishStatus {
    NotPublished,
    Private,
    UpToDate {
        version: String,
        has_uncommitted_changes: bool,
    },
    Outdated {
        local_version: String,
        published_version: String,
    },
    Ahead {
        local_version: String,
        published_version: String,
    },
}

impl PublishStatus {
    pub fn is_success(&self, strict: bool) -> bool {
        match self {
            PublishStatus::UpToDate {
                has_uncommitted_changes,
                ..
            } => !(strict && *has_uncommitted_changes),
            _ => false,
        }
    }

    pub fn format_message(&self, name: &str, local_version: &str) -> String {
        match self {
            PublishStatus::NotPublished => {
                format!("{name} {local_version} is not published")
            }
            PublishStatus::Private => {
                format!("{name} {local_version} is not published (private crate: publish = false)")
            }
            PublishStatus::UpToDate {
                version,
                has_uncommitted_changes,
            } => {
                if *has_uncommitted_changes {
                    format!("{name} {version} is published and up to date (uncommitted changes)")
                } else {
                    format!("{name} {version} is published and up to date")
                }
            }
            PublishStatus::Outdated {
                published_version, ..
            } => {
                format!(
                    "{name} {local_version} is published, but outdated (latest published is {published_version})"
                )
            }
            PublishStatus::Ahead {
                published_version, ..
            } => {
                format!(
                    "{name} {local_version} is published, but local version is ahead (latest published is {published_version})"
                )
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct Metadata {
    packages: Vec<PackageMetadata>,
    #[serde(default)]
    workspace_members: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct PackageMetadata {
    name: String,
    version: String,
    manifest_path: String,
    publish: Option<Vec<String>>,
}

pub fn find_manifest(start_dir: &Path) -> Option<PathBuf> {
    let abs_path = if let Ok(canon) = fs::canonicalize(start_dir) {
        canon
    } else if start_dir.is_relative() {
        std::env::current_dir().ok()?.join(start_dir)
    } else {
        start_dir.to_path_buf()
    };

    let mut current = if abs_path.is_file() {
        if abs_path.file_name().is_some_and(|n| n == "Cargo.toml") {
            return Some(abs_path);
        }
        abs_path.parent()?.to_path_buf()
    } else {
        abs_path
    };

    loop {
        let manifest = current.join("Cargo.toml");
        if manifest.is_file() {
            return Some(manifest);
        }
        if !current.pop() {
            break;
        }
    }
    None
}

pub fn parse_published_version_from_output(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("version:") {
            let ver = rest.split_whitespace().next().unwrap_or("").trim();
            if !ver.is_empty() {
                return Some(ver.to_string());
            }
        }
    }
    None
}

pub fn compare_versions(local: &str, published: &str) -> std::cmp::Ordering {
    if let (Ok(local_v), Ok(pub_v)) = (Version::parse(local), Version::parse(published)) {
        local_v.cmp(&pub_v)
    } else if local == published {
        std::cmp::Ordering::Equal
    } else {
        // Fallback simple comparison if not valid semver
        local.cmp(published)
    }
}

pub fn check_git_uncommitted(repo_dir: &Path) -> bool {
    let output = match Command::new("git")
        .arg("-C")
        .arg(repo_dir)
        .args(["status", "--porcelain"])
        .output()
    {
        Ok(out) => out,
        Err(_) => return false,
    };

    if !output.status.success() {
        return false;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    !stdout.trim().is_empty()
}

fn query_registry_published_version(name: &str, registry: &str) -> Result<Option<String>, String> {
    let mut cmd = Command::new("cargo");
    cmd.arg("info")
        .arg("-q")
        .arg("--registry")
        .arg(registry)
        .arg(name)
        // Run from temp dir to prevent matching local packages
        .current_dir(std::env::temp_dir());

    let output = cmd
        .output()
        .map_err(|e| format!("Failed to execute `cargo info`: {e}"))?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(parse_published_version_from_output(&stdout))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("could not find") || stderr.contains("not found") {
            Ok(None)
        } else {
            Err(stderr.trim().to_string())
        }
    }
}

fn check_package(package: &PackageMetadata, registry: &str) -> Result<PublishStatus, String> {
    // Check if package is marked as private
    if let Some(ref registries) = package.publish {
        let allowed = registries.iter().any(|r| {
            r == registry || (registry == "crates-io" && (r == "crates.io" || r == "crates-io"))
        });
        if !allowed {
            return Ok(PublishStatus::Private);
        }
    }

    let published_version = query_registry_published_version(&package.name, registry)?;
    let Some(published_version) = published_version else {
        return Ok(PublishStatus::NotPublished);
    };

    let manifest_path = Path::new(&package.manifest_path);
    let package_dir = manifest_path.parent().unwrap_or(Path::new("."));
    let has_uncommitted_changes = check_git_uncommitted(package_dir);

    match compare_versions(&package.version, &published_version) {
        std::cmp::Ordering::Equal => Ok(PublishStatus::UpToDate {
            version: package.version.clone(),
            has_uncommitted_changes,
        }),
        std::cmp::Ordering::Less => Ok(PublishStatus::Outdated {
            local_version: package.version.clone(),
            published_version,
        }),
        std::cmp::Ordering::Greater => Ok(PublishStatus::Ahead {
            local_version: package.version.clone(),
            published_version,
        }),
    }
}

fn run(args: Args) -> Result<bool, String> {
    let manifest_path = find_manifest(&args.path).ok_or_else(|| {
        format!(
            "Could not find `Cargo.toml` in `{}` or any parent directory",
            args.path.display()
        )
    })?;

    let canonical_manifest = manifest_path
        .canonicalize()
        .map_err(|e| format!("Failed to canonicalize `{}`: {e}", manifest_path.display()))?;

    let output = Command::new("cargo")
        .arg("metadata")
        .arg("--no-deps")
        .arg("--format-version")
        .arg("1")
        .arg("--manifest-path")
        .arg(&canonical_manifest)
        .output()
        .map_err(|e| format!("Failed to run `cargo metadata`: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("`cargo metadata` failed: {}", stderr.trim()));
    }

    let metadata: Metadata = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Failed to parse metadata JSON: {e}"))?;

    // Find the target packages:
    // If canonical_manifest directly matches a package manifest, check that package.
    // Otherwise (e.g. virtual workspace), check all workspace members.
    let packages_to_check: Vec<&PackageMetadata> = {
        let matching_pkg = metadata.packages.iter().find(|p| {
            Path::new(&p.manifest_path)
                .canonicalize()
                .map(|p| p == canonical_manifest)
                .unwrap_or(false)
        });

        if let Some(pkg) = matching_pkg {
            vec![pkg]
        } else {
            // Workspace root
            metadata
                .packages
                .iter()
                .filter(|p| {
                    metadata
                        .workspace_members
                        .iter()
                        .any(|m| m.contains(&p.name) || m.contains(&p.manifest_path))
                })
                .collect()
        }
    };

    if packages_to_check.is_empty() {
        return Err("No package found to check in manifest".to_string());
    }

    let mut all_success = true;

    for pkg in packages_to_check {
        let status = check_package(pkg, &args.registry)?;
        if !status.is_success(args.strict) {
            all_success = false;
        }

        if !args.quiet {
            println!("{}", status.format_message(&pkg.name, &pkg.version));
        }
    }

    Ok(all_success)
}

fn main() -> ExitCode {
    let args = Args::parse();
    let quiet = args.quiet;

    match run(args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(err) => {
            if !quiet {
                eprintln!("Error: {err}");
            }
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_args_parsing() {
        let args = Args::try_parse_from(["is-published"]).unwrap();
        assert_eq!(args.path, PathBuf::from("."));
        assert_eq!(args.registry, "crates-io");
        assert!(!args.quiet);
        assert!(!args.strict);

        let args = Args::try_parse_from([
            "is-published",
            "/some/path",
            "-q",
            "--strict",
            "-r",
            "custom",
        ])
        .unwrap();
        assert_eq!(args.path, PathBuf::from("/some/path"));
        assert_eq!(args.registry, "custom");
        assert!(args.quiet);
        assert!(args.strict);
    }

    #[test]
    fn test_parse_published_version_from_output() {
        let sample = "clap #argument #cli\nversion: 4.6.7\nlicense: MIT\n";
        assert_eq!(
            parse_published_version_from_output(sample),
            Some("4.6.7".to_string())
        );

        let sample_with_latest = "clap\nversion: 4.0.0 (latest 4.6.7)\nlicense: MIT\n";
        assert_eq!(
            parse_published_version_from_output(sample_with_latest),
            Some("4.0.0".to_string())
        );

        let empty_sample = "no version info here\n";
        assert_eq!(parse_published_version_from_output(empty_sample), None);
    }

    #[test]
    fn test_compare_versions() {
        assert_eq!(
            compare_versions("1.0.0", "1.0.0"),
            std::cmp::Ordering::Equal
        );
        assert_eq!(
            compare_versions("1.1.0", "1.0.0"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(compare_versions("0.9.0", "1.0.0"), std::cmp::Ordering::Less);
        assert_eq!(
            compare_versions("1.0.0-alpha.1", "1.0.0"),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn test_status_messages() {
        let s1 = PublishStatus::NotPublished;
        assert_eq!(
            s1.format_message("my-crate", "0.1.0"),
            "my-crate 0.1.0 is not published"
        );
        assert!(!s1.is_success(false));

        let s2 = PublishStatus::Private;
        assert_eq!(
            s2.format_message("my-crate", "0.1.0"),
            "my-crate 0.1.0 is not published (private crate: publish = false)"
        );
        assert!(!s2.is_success(false));

        let s3 = PublishStatus::UpToDate {
            version: "0.1.0".to_string(),
            has_uncommitted_changes: false,
        };
        assert_eq!(
            s3.format_message("my-crate", "0.1.0"),
            "my-crate 0.1.0 is published and up to date"
        );
        assert!(s3.is_success(false));
        assert!(s3.is_success(true));

        let s4 = PublishStatus::UpToDate {
            version: "0.1.0".to_string(),
            has_uncommitted_changes: true,
        };
        assert_eq!(
            s4.format_message("my-crate", "0.1.0"),
            "my-crate 0.1.0 is published and up to date (uncommitted changes)"
        );
        assert!(s4.is_success(false));
        assert!(!s4.is_success(true));

        let s5 = PublishStatus::Outdated {
            local_version: "0.1.0".to_string(),
            published_version: "0.2.0".to_string(),
        };
        assert_eq!(
            s5.format_message("my-crate", "0.1.0"),
            "my-crate 0.1.0 is published, but outdated (latest published is 0.2.0)"
        );
        assert!(!s5.is_success(false));

        let s6 = PublishStatus::Ahead {
            local_version: "0.2.0".to_string(),
            published_version: "0.1.0".to_string(),
        };
        assert_eq!(
            s6.format_message("my-crate", "0.2.0"),
            "my-crate 0.2.0 is published, but local version is ahead (latest published is 0.1.0)"
        );
        assert!(!s6.is_success(false));
    }

    #[test]
    fn test_find_manifest() {
        let temp_dir =
            std::env::temp_dir().join(format!("is_published_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(temp_dir.join("sub/deep")).unwrap();

        let cargo_toml = temp_dir.join("Cargo.toml");
        std::fs::write(
            &cargo_toml,
            "[package]\nname = \"test\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        // Direct directory
        assert_eq!(find_manifest(&temp_dir), Some(cargo_toml.clone()));
        // Direct file
        assert_eq!(find_manifest(&cargo_toml), Some(cargo_toml.clone()));
        // Subdirectory walk-up
        assert_eq!(
            find_manifest(&temp_dir.join("sub/deep")),
            Some(cargo_toml.clone())
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
