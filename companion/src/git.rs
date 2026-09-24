//! Lightweight git reporting.
//!
//! Prefers the `git` CLI because it needs no server round-trip and works for
//! any project directory, git-backed or not. The OpenCode VCS API is used
//! separately for the richer in-game Git tab.

use std::path::Path;
use std::process::Command;

/// A compact summary of a repository's state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitSummary {
    pub branch: String,
    pub ahead: u32,
    pub behind: u32,
    pub changed_files: usize,
    pub last_commits: Vec<String>,
}

impl GitSummary {
    /// One-line rendering used in prompts and the in-game status line.
    pub fn one_line(&self) -> String {
        let mut parts = vec![format!("branch {}", self.branch)];
        if self.ahead > 0 {
            parts.push(format!("{} ahead", self.ahead));
        }
        if self.behind > 0 {
            parts.push(format!("{} behind", self.behind));
        }
        if self.changed_files > 0 {
            parts.push(format!("{} changed", self.changed_files));
        } else {
            parts.push("clean".to_string());
        }
        if let Some(last) = self.last_commits.first() {
            parts.push(format!("last: {last}"));
        }
        parts.join(", ")
    }

    /// Multi-line rendering for the in-game Git tab.
    pub fn multi_line(&self) -> String {
        let mut out = format!("branch: {}\n", self.branch);
        out.push_str(&format!(
            "ahead: {}, behind: {}, changed: {}\n",
            self.ahead, self.behind, self.changed_files
        ));
        if !self.last_commits.is_empty() {
            out.push_str("recent commits:\n");
            for commit in &self.last_commits {
                out.push_str(&format!("  {commit}\n"));
            }
        }
        out
    }
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Summarise a directory if it is inside a git work tree.
pub fn summarize(dir: &Path) -> Option<GitSummary> {
    if !dir.exists() {
        return None;
    }
    let branch = git(dir, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if branch.is_empty() {
        return None;
    }

    let mut summary = GitSummary {
        branch,
        ..Default::default()
    };

    if let Some(status) = git(dir, &["status", "--porcelain"]) {
        summary.changed_files = status.lines().filter(|l| !l.trim().is_empty()).count();
    }

    if let Some(counts) = git(dir, &["rev-list", "--left-right", "--count", "@{upstream}...HEAD"]) {
        let mut it = counts.split_whitespace();
        summary.behind = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        summary.ahead = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    }

    if let Some(log) = git(dir, &["log", "--oneline", "-3", "--no-decorate"]) {
        summary.last_commits = log.lines().map(|l| l.trim().to_string()).collect();
    }

    Some(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_clean_summary() {
        let summary = GitSummary {
            branch: "main".into(),
            changed_files: 0,
            last_commits: vec!["abc123 do a thing".into()],
            ..Default::default()
        };
        let line = summary.one_line();
        assert!(line.contains("branch main"));
        assert!(line.contains("clean"));
        assert!(line.contains("abc123"));
    }

    #[test]
    fn missing_directory_is_none() {
        assert!(summarize(Path::new("/definitely/not/a/dir")).is_none());
    }
}
