//! Integration pins for the workspace codepoint_scan binary.
//!
//! Moved from root tests/setup.rs in slice DB-S4 when codepoint_scan
//! moved into its own non-published workspace crate under DB-25.

use std::path::{Path, PathBuf};
use std::process::Command;

struct ScanOutput {
    status: i32,
    stdout: String,
    stderr: String,
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dibs-scan-test-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn scan_tree(root: &Path) -> ScanOutput {
    let output = Command::new(env!("CARGO_BIN_EXE_codepoint_scan"))
        .arg(root)
        .output()
        .unwrap();
    ScanOutput {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn scan_tree_with_surplus(root: &Path, surplus: &Path) -> ScanOutput {
    let output = Command::new(env!("CARGO_BIN_EXE_codepoint_scan"))
        .arg(root)
        .arg(surplus)
        .output()
        .unwrap();
    ScanOutput {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn scan_current_dir(root: &Path) -> ScanOutput {
    let output = Command::new(env!("CARGO_BIN_EXE_codepoint_scan"))
        .current_dir(root)
        .output()
        .unwrap();
    ScanOutput {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

#[test]
fn codepoint_scan_reports_zero_for_the_repo() {
    let root = repo_root();
    assert!(root.join("AGENTS.md").is_file());
    assert!(root.join("CONTRACT.md").is_file());
    let result = scan_tree(&root);
    assert_eq!(result.status, 0, "{}", result.stdout);
    assert!(result.stdout.contains("em dash U+2014 count 0"));
    assert!(result.stdout.contains("en dash U+2013 count 0"));
}

#[test]
fn codepoint_scan_refuses_an_argument_after_the_root() {
    let dir = temp_dir("surplus-argument");
    std::fs::write(dir.join("clean.md"), "clean").unwrap();
    let surplus = temp_dir("surplus-other");
    let result = scan_tree_with_surplus(&dir, &surplus);
    assert_eq!(result.status, 2, "{}", result.stdout);
    assert!(
        result
            .stderr
            .contains("scan error, the scan takes at most one argument, the scan root"),
        "{}",
        result.stderr
    );
}

#[test]
fn codepoint_scan_refuses_an_extra_argument_before_scanning() {
    let dir = temp_dir("surplus-before-scan");
    std::fs::write(dir.join("note.md"), "a\u{2014}b").unwrap();
    let surplus = temp_dir("surplus-before-scan-other");
    let result = scan_tree_with_surplus(&dir, &surplus);
    assert_eq!(result.status, 2, "{}", result.stdout);
    assert!(
        result
            .stderr
            .contains("scan error, the scan takes at most one argument, the scan root"),
        "{}",
        result.stderr
    );
    assert!(!result.stdout.contains("em dash U+2014 count"));
}

#[test]
fn codepoint_scan_with_no_argument_scans_the_current_directory() {
    let dir = temp_dir("no-argument");
    std::fs::write(dir.join("note.md"), "a\u{2014}b").unwrap();
    let result = scan_current_dir(&dir);
    assert_eq!(result.status, 1);
    assert!(result.stdout.contains("em dash U+2014 count 1"));
}

#[test]
fn codepoint_scan_counts_an_em_dash() {
    let dir = temp_dir("em-dash");
    std::fs::write(dir.join("note.md"), "a\u{2014}b").unwrap();
    let result = scan_tree(&dir);
    assert_eq!(result.status, 1);
    assert!(result.stdout.contains("em dash U+2014 count 1"));
    assert!(result.stdout.contains("note.md"));
}

#[test]
fn codepoint_scan_counts_an_en_dash() {
    let dir = temp_dir("en-dash");
    std::fs::write(dir.join("note.md"), "a\u{2013}b").unwrap();
    let result = scan_tree(&dir);
    assert_eq!(result.status, 1);
    assert!(result.stdout.contains("en dash U+2013 count 1"));
}

#[test]
fn codepoint_scan_scans_a_root_under_an_excluded_ancestor() {
    let dir = temp_dir("excluded-ancestor").join("venv").join("tree");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("note.md"), "a\u{2014}b").unwrap();
    let result = scan_tree(&dir);
    assert_eq!(result.status, 1);
    assert!(result.stdout.contains("em dash U+2014 count 1"));
}

#[test]
fn codepoint_scan_skips_excluded_dirs_below_the_root() {
    let dir = temp_dir("excluded-dirs");
    let cache = dir.join("build");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join("note.md"), "a\u{2014}b").unwrap();
    std::fs::write(dir.join("clean.md"), "clean").unwrap();
    let result = scan_tree(&dir);
    assert_eq!(result.status, 0);
    assert!(result.stdout.contains("em dash U+2014 count 0"));
}

#[test]
fn codepoint_scan_fails_loudly_on_undecodable_input() {
    let dir = temp_dir("undecodable");
    std::fs::write(dir.join("note.md"), b"\xff\xfe not utf-8").unwrap();
    let result = scan_tree(&dir);
    assert_eq!(result.status, 2);
    assert!(result.stderr.contains("note.md"));
}

#[test]
fn codepoint_scan_counts_a_dash_in_a_file_with_no_suffix() {
    let dir = temp_dir("no-suffix");
    std::fs::write(dir.join("LICENSE"), "a\u{2014}b").unwrap();
    let result = scan_tree(&dir);
    assert_eq!(result.status, 1);
    assert!(result.stdout.contains("em dash U+2014 count 1"));
}

#[test]
fn codepoint_scan_counts_a_dash_in_an_uppercase_suffix_file() {
    let dir = temp_dir("upper-suffix");
    std::fs::write(dir.join("UPPER.MD"), "a\u{2014}b").unwrap();
    let result = scan_tree(&dir);
    assert_eq!(result.status, 1);
    assert!(result.stdout.contains("em dash U+2014 count 1"));
}

#[test]
fn codepoint_scan_skips_a_file_with_an_uppercase_binary_suffix() {
    let dir = temp_dir("upper-binary");
    std::fs::write(dir.join("FAKE.PNG"), "a\u{2014}b").unwrap();
    let result = scan_tree(&dir);
    assert_eq!(result.status, 0);
    assert!(result.stdout.contains("em dash U+2014 count 0"));
    assert!(result.stdout.contains("en dash U+2013 count 0"));
}

#[cfg(unix)]
#[test]
fn codepoint_scan_takes_a_non_unicode_root_without_panicking() {
    use std::os::unix::ffi::OsStringExt;

    // The scanner read its root through std::env::args as
    // first built, which panics on an argument that is not
    // valid Unicode, where the root is a path and stands
    // carried unconverted, the shape the command line's
    // registry path takes. DB-S1 guards `create_dir_all` on
    // filesystems such as macOS APFS that reject non UTF-8
    // directory entries on disk, skipping only the staged
    // directory while still exercising the raw OsString scan
    // root error path below.
    let dir = temp_dir("non-unicode-root");
    let odd_dir = dir.join(std::ffi::OsString::from_vec(vec![0xff]));
    if std::fs::create_dir_all(&odd_dir).is_ok() {
        std::fs::write(odd_dir.join("clean.md"), "clean").unwrap();
        let result = scan_tree(&odd_dir);
        assert_eq!(result.status, 0, "{}", result.stderr);
        assert!(result.stdout.contains("em dash U+2014 count 0"));
        assert!(!result.stderr.contains("panicked"), "{}", result.stderr);
    }
    let missing = dir.join(std::ffi::OsString::from_vec(vec![0xfe]));
    let result = scan_tree(&missing);
    assert_eq!(result.status, 2, "{}", result.stderr);
    assert!(result.stderr.contains("scan error"), "{}", result.stderr);
    assert!(!result.stderr.contains("panicked"), "{}", result.stderr);
}

#[cfg(unix)]
#[test]
fn codepoint_scan_with_a_closed_stdout_pipe_exits_2_without_a_panic() {
    use std::process::Stdio;

    let dir = temp_dir("scan-epipe");
    std::fs::write(dir.join("clean.md"), "clean").unwrap();
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let output = Command::new(env!("CARGO_BIN_EXE_codepoint_scan"))
        .arg(&dir)
        .stdout(Stdio::from(writer))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert!(!stderr.contains("failed printing to stdout"), "{stderr}");
}

#[cfg(unix)]
#[test]
fn codepoint_scan_with_an_unwritable_stdout_exits_2_without_a_panic() {
    use std::process::Stdio;

    let dir = temp_dir("scan-full");
    std::fs::write(dir.join("clean.md"), "clean").unwrap();
    let full = match std::fs::OpenOptions::new().write(true).open("/dev/full") {
        Ok(file) => file,
        Err(_) => return,
    };
    let output = Command::new(env!("CARGO_BIN_EXE_codepoint_scan"))
        .arg(&dir)
        .stdout(Stdio::from(full))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert!(!stderr.contains("failed printing to stdout"), "{stderr}");
}
