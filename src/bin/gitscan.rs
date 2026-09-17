use clap::Parser;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(default_value = ".")]
    path: PathBuf,

    #[arg(
        short = 'n',
        long = "newest",
        help = "Sort folders in most recently modified order"
    )]
    newest: bool,
}

fn find_git_repos(dir: &Path, repos: &mut Vec<PathBuf>) {
    if dir.join(".git").exists() {
        repos.push(dir.to_path_buf());
    }

    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        // Don't follow symlinks: avoids loops and scanning outside the tree.
        if !file_type.is_dir() || file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if path.file_name().is_some_and(|n| n == ".git") {
            continue;
        }
        find_git_repos(&path, repos);
    }
}

fn git_stdout(repo: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn sort_repos(repos: &mut [PathBuf], newest: bool) {
    if newest {
        repos.sort_by_cached_key(|repo| {
            (
                std::cmp::Reverse(fs::metadata(repo).and_then(|m| m.modified()).ok()),
                repo.clone(),
            )
        });
    } else {
        repos.sort();
    }
}

fn main() {
    let args = Args::parse();
    let root = args.path;

    let mut repos = Vec::new();
    find_git_repos(&root, &mut repos);
    sort_repos(&mut repos, args.newest);

    println!(
        "Scanned {} git repo(s) under {}\n",
        repos.len(),
        root.display()
    );

    for repo in &repos {
        let mut statuses = Vec::new();

        let remotes = git_stdout(repo, &["remote"]).unwrap_or_default();
        if remotes.trim().is_empty() {
            statuses.push("no remote");
        }

        let status = git_stdout(repo, &["status", "--porcelain"]).unwrap_or_default();
        if !status.trim().is_empty() {
            statuses.push("uncommitted changes");
        }

        if !statuses.is_empty() {
            println!("{}: {}", repo.display(), statuses.join(", "));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_args_parsing() {
        let args = Args::try_parse_from(["gitscan"]).unwrap();
        assert_eq!(args.path, PathBuf::from("."));
        assert!(!args.newest);

        let args = Args::try_parse_from(["gitscan", "-n"]).unwrap();
        assert_eq!(args.path, PathBuf::from("."));
        assert!(args.newest);

        let args = Args::try_parse_from(["gitscan", "--newest", "/some/path"]).unwrap();
        assert_eq!(args.path, PathBuf::from("/some/path"));
        assert!(args.newest);

        let args = Args::try_parse_from(["gitscan", "/some/path", "-n"]).unwrap();
        assert_eq!(args.path, PathBuf::from("/some/path"));
        assert!(args.newest);
    }

    #[test]
    fn test_sort_repos() {
        let temp_dir = std::env::temp_dir().join(format!("gitscan_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let repo_a = temp_dir.join("repo_a");
        let repo_b = temp_dir.join("repo_b");
        let repo_c = temp_dir.join("repo_c");

        fs::create_dir(&repo_a).unwrap();
        fs::create_dir(&repo_b).unwrap();
        fs::create_dir(&repo_c).unwrap();

        let set_mtime = |path: &Path, mtime_secs: i64| {
            let ft = std::time::SystemTime::UNIX_EPOCH
                + std::time::Duration::from_secs(mtime_secs as u64);
            let file = fs::File::open(path).unwrap();
            file.set_modified(ft).unwrap();
        };

        set_mtime(&repo_a, 2000);
        set_mtime(&repo_b, 1000);
        set_mtime(&repo_c, 3000);

        let mut repos = vec![repo_a.clone(), repo_b.clone(), repo_c.clone()];

        // Default sort (alphabetical)
        sort_repos(&mut repos, false);
        assert_eq!(repos, vec![repo_a.clone(), repo_b.clone(), repo_c.clone()]);

        // Newest sort (mtime descending: c -> a -> b)
        sort_repos(&mut repos, true);
        assert_eq!(repos, vec![repo_c.clone(), repo_a.clone(), repo_b.clone()]);

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
