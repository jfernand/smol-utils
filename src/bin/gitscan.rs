use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    let output = Command::new("git").arg("-C").arg(repo).args(args).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| ".".to_string());
    let root = PathBuf::from(root);

    let mut repos = Vec::new();
    find_git_repos(&root, &mut repos);
    repos.sort();

    println!("Scanned {} git repo(s) under {}\n", repos.len(), root.display());

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
