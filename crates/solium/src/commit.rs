//! The commit a build is made from, for `solium --version` (#156).
//!
//! Read from git's own files rather than by running git, because a build needs
//! the checkout and not git: the build image has none. `build.rs` includes this
//! file by `#[path]` and runs it; the compositor compiles it only for its
//! tests, `commit::tests`.

use std::path::{Path, PathBuf};

/// The commit, shortened as `git log --oneline` shortens it, and the files
/// whose change means another commit, for `cargo:rerun-if-changed`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Commit {
    pub(crate) short: String,
    pub(crate) watch: Vec<PathBuf>,
}

/// The commit checked out at `root`, the top of a checkout or of a worktree;
/// `None` when `root` is neither, or its branch has no commit yet.
/// `tests::a_branch_names_the_commit_its_file_holds`,
/// `tests::no_checkout_and_an_unborn_branch_name_no_commit`.
pub(crate) fn find(root: &Path) -> Option<Commit> {
    let dot_git = root.join(".git");
    // A worktree's `.git` is a file naming its own git directory, and its
    // `commondir` is where the branches are.
    // `tests::a_worktree_reads_its_branch_from_the_common_directory`.
    let own = if dot_git.is_file() {
        let text = std::fs::read_to_string(&dot_git).ok()?;
        root.join(text.trim().strip_prefix("gitdir:")?.trim())
    } else {
        dot_git
    };
    let common = match std::fs::read_to_string(own.join("commondir")) {
        Ok(text) => own.join(text.trim()),
        Err(_) => own.clone(),
    };
    let head = own.join("HEAD");
    let text = std::fs::read_to_string(&head).ok()?;
    let mut watch = vec![head];
    // Whatever moves HEAD writes here, a commit on a packed branch included,
    // which writes a branch file that was not there to watch.
    // `tests::a_packed_branch_is_found_in_packed_refs`.
    let log = own.join("logs/HEAD");
    if log.is_file() {
        watch.push(log);
    }
    let hash = match text.trim().strip_prefix("ref:") {
        // A detached HEAD holds the commit itself.
        // `tests::a_detached_head_is_its_own_commit`.
        None => text.trim().to_owned(),
        Some(branch) => {
            let branch = branch.trim();
            let loose = common.join(branch);
            if let Ok(text) = std::fs::read_to_string(&loose) {
                watch.push(loose);
                text.trim().to_owned()
            } else {
                // `git gc` folds branches into `packed-refs`.
                // `tests::a_packed_branch_is_found_in_packed_refs`.
                let packed = common.join("packed-refs");
                let text = std::fs::read_to_string(&packed).ok()?;
                let hash = text.lines().find_map(|line| {
                    let (hash, name) = line.split_once(' ')?;
                    (name == branch).then(|| hash.to_owned())
                })?;
                watch.push(packed);
                hash
            }
        }
    };
    let short = hash.get(..7)?;
    short
        .chars()
        .all(|c| c.is_ascii_hexdigit())
        .then(|| Commit {
            short: short.to_owned(),
            watch,
        })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Commit, find};

    const HASH: &str = "dfc95ce0123456789abcdef0123456789abcdef0";

    /// Cleared first, because these are named after the test and the process
    /// and a second run of the same test would otherwise find its leftovers.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("solium-commit-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temporary directory");
        dir
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("its directory");
        std::fs::write(path, text).expect("writing a git file");
    }

    #[test]
    fn a_branch_names_the_commit_its_file_holds() {
        let root = scratch("branch");
        write(&root.join(".git/HEAD"), "ref: refs/heads/main\n");
        write(&root.join(".git/refs/heads/main"), &format!("{HASH}\n"));
        assert_eq!(
            find(&root),
            Some(Commit {
                short: "dfc95ce".to_owned(),
                watch: vec![root.join(".git/HEAD"), root.join(".git/refs/heads/main")],
            })
        );
    }

    /// A branch `git gc` folded into `packed-refs` has no file of its own, so
    /// HEAD's log is watched too: a commit on it writes a branch file that was
    /// not there to watch.
    #[test]
    fn a_packed_branch_is_found_in_packed_refs() {
        let root = scratch("packed");
        write(&root.join(".git/HEAD"), "ref: refs/heads/main\n");
        write(&root.join(".git/logs/HEAD"), "a commit, logged\n");
        write(
            &root.join(".git/packed-refs"),
            &format!(
                "# pack-refs with: peeled fully-peeled sorted\n\
                 0123456789abcdef0123456789abcdef01234567 refs/heads/other\n\
                 {HASH} refs/heads/main\n"
            ),
        );
        assert_eq!(
            find(&root),
            Some(Commit {
                short: "dfc95ce".to_owned(),
                watch: vec![
                    root.join(".git/HEAD"),
                    root.join(".git/logs/HEAD"),
                    root.join(".git/packed-refs"),
                ],
            })
        );
    }

    #[test]
    fn a_detached_head_is_its_own_commit() {
        let root = scratch("detached");
        write(&root.join(".git/HEAD"), &format!("{HASH}\n"));
        assert_eq!(
            find(&root),
            Some(Commit {
                short: "dfc95ce".to_owned(),
                watch: vec![root.join(".git/HEAD")],
            })
        );
    }

    /// A worktree's `.git` is a file naming its own git directory, whose
    /// `commondir` is where the branches are.
    #[test]
    fn a_worktree_reads_its_branch_from_the_common_directory() {
        let root = scratch("worktree");
        let checkout = root.join("checkout");
        let main = root.join("main/.git");
        let own = main.join("worktrees/topic");
        write(
            &checkout.join(".git"),
            &format!("gitdir: {}\n", own.display()),
        );
        write(&own.join("HEAD"), "ref: refs/heads/topic\n");
        write(&own.join("commondir"), "../..\n");
        write(&main.join("refs/heads/topic"), HASH);
        let found = find(&checkout).expect("the worktree's commit");
        assert_eq!(found.short, "dfc95ce");
        let watched: Vec<PathBuf> = found
            .watch
            .iter()
            .map(|path| path.canonicalize().expect("a watched file exists"))
            .collect();
        assert_eq!(
            watched,
            vec![
                own.join("HEAD").canonicalize().expect("HEAD"),
                main.join("refs/heads/topic")
                    .canonicalize()
                    .expect("the branch"),
            ]
        );
    }

    /// A source tarball has no `.git`, and a branch with no commit yet has no
    /// file: neither names a commit, so `--version` is the version alone.
    #[test]
    fn no_checkout_and_an_unborn_branch_name_no_commit() {
        assert_eq!(find(&scratch("tarball")), None);
        let unborn = scratch("unborn");
        write(&unborn.join(".git/HEAD"), "ref: refs/heads/main\n");
        assert_eq!(find(&unborn), None);
    }
}
