//! The setup pins, translated from the Python setup suite the repo
//! was seeded with, brought up to the current fleet standard. Each
//! test names the same fact its Python original named, with the
//! strengthenings the fleet folded in after dibs's Round 1 suite
//! was written, three of them landing with this port: the job level
//! if false pin, the wrong arc label pin, and the CI credential
//! hygiene pin. Two facts changed with the Rust conversion and the
//! change is stated where it lands. The CI gate commands are the
//! Rust gate commands of amended obligation S2, and the contract
//! substance pin asserts the amended S2 phrases.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

const MARKER: &str = "<!-- repo annex below this line -->";
const GATE_COMMAND: &str = "cargo test";
const SCAN_COMMAND: &str = "cargo run --bin codepoint_scan";
const FMT_COMMAND: &str = "cargo fmt --check";
const CLIPPY_COMMAND: &str = "cargo clippy --all-targets -- -D warnings";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(name: &str) -> String {
    std::fs::read_to_string(root().join(name)).unwrap()
}

fn temp_dir(tag: &str) -> PathBuf {
    common::temp_dir("dibs-setup-test-", tag)
}

#[test]
fn seed_files_exist() {
    for name in ["README.md", "LICENSE", ".gitignore"] {
        assert!(root().join(name).is_file(), "{name}");
    }
}

#[test]
fn license_starts_with_mit_heading() {
    assert!(read("LICENSE").starts_with("MIT License\n"));
}

#[test]
fn agents_has_marker_and_dibs_annex_below_it() {
    let text = read("AGENTS.md");
    assert!(text.contains(MARKER));
    let annex = text.split(MARKER).nth(1).unwrap();
    assert!(annex.contains("dibs"));
    assert!(annex.contains("CONTRACT.md"));
    assert!(annex.contains("expiring claim-lease protocol"));
}

#[test]
fn agents_canonical_part_carries_program_headings() {
    let text = read("AGENTS.md");
    let canonical = text.split(MARKER).next().unwrap();
    assert!(canonical.starts_with("# AGENTS.md\n"));
    assert!(canonical.contains("## The round discipline"));
    assert!(canonical.contains("## Contract first"));
    assert!(canonical.contains("39 repo storefront perfection program"));
}

#[test]
fn contract_names_five_setup_obligations() {
    let text = read("CONTRACT.md");
    for name in ["S1", "S2", "S3", "S4", "S5"] {
        assert!(
            text.contains(&format!("Obligation {name} covers")),
            "{name}"
        );
    }
    assert!(!text.contains("Obligation S6"));
}

fn status_section(text: &str) -> &str {
    let start = text.find("\n## Status\n").unwrap() + "\n## Status\n".len();
    let rest = &text[start..];
    match rest.find("\n## ") {
        Some(end) => &rest[..end],
        None => rest,
    }
}

#[test]
fn readme_status_states_one_hundred_of_one_hundred() {
    let text = read("README.md");
    assert!(text.contains("## Status"));
    let status = status_section(&text);
    // The full sentence, never the bare count, because the
    // bare count is a substring of every two digit count that
    // ends in the same digit and a pin on it would pass over
    // a wrong statement of the closed rounds.
    assert!(status.contains("Closed perfection rounds are 100 of 100."));
    assert!(status.contains("c1533bbc5fcfb803c1511d677d36c3c3925f075d"));
}

fn flush_step(step_lines: &mut Vec<String>, commands: &mut Vec<String>) {
    if step_lines.is_empty() {
        return;
    }
    let mut disabled = false;
    let mut runs = Vec::new();
    for stripped in step_lines.iter() {
        if let Some(value) = stripped.strip_prefix("if:") {
            let value = value
                .trim()
                .trim_matches(|c| c == '\'' || c == '"')
                .to_lowercase();
            if value == "false" {
                disabled = true;
            }
        }
        if let Some(pos) = stripped.find("run:") {
            runs.push(stripped[pos + "run:".len()..].trim().to_string());
        }
    }
    if !disabled {
        commands.extend(runs);
    }
    step_lines.clear();
}

fn job_section(ci_text: &str, target_job: &str) -> String {
    let mut section = String::new();
    let mut in_target = false;
    let target_header = format!("{target_job}:");
    for line in ci_text.lines() {
        let head = line.split(" #").next().unwrap_or("");
        let top_level = !head.is_empty()
            && !head.starts_with(' ')
            && !head.starts_with('#')
            && head.trim_end().ends_with(':');
        if top_level {
            if in_target {
                break;
            }
            continue;
        }
        let job_line =
            head.starts_with("  ") && !head.starts_with("   ") && head.trim_end().ends_with(':');
        if job_line {
            if in_target {
                break;
            }
            in_target = head.trim() == target_header;
            continue;
        }
        if in_target {
            section.push_str(line);
            section.push('\n');
        }
    }
    section
}

fn run_commands_for_job(ci_text: &str, target_job: &str) -> Vec<String> {
    let section = job_section(ci_text, target_job);
    if section.is_empty() {
        return Vec::new();
    }
    let mut commands = Vec::new();
    let mut step_lines: Vec<String> = Vec::new();
    let mut in_steps = false;
    let mut job_disabled = false;
    for line in section.lines() {
        let stripped = line.trim();
        if stripped.starts_with('#') {
            continue;
        }
        if !in_steps && stripped.starts_with("if:") {
            let value = stripped
                .split_once(':')
                .map(|(_, rest)| rest)
                .unwrap_or_default()
                .trim()
                .trim_matches(|c| c == '\'' || c == '"')
                .to_lowercase();
            if value == "false" {
                job_disabled = true;
            }
            continue;
        }
        if stripped == "steps:" {
            in_steps = true;
            continue;
        }
        if in_steps && stripped.starts_with("- ") {
            flush_step(&mut step_lines, &mut commands);
            step_lines.push(stripped[2..].trim().to_string());
            continue;
        }
        if in_steps {
            step_lines.push(stripped.to_string());
        }
    }
    flush_step(&mut step_lines, &mut commands);
    if job_disabled {
        return Vec::new();
    }
    commands
}

fn run_commands(ci_text: &str) -> Vec<String> {
    run_commands_for_job(ci_text, "test")
}

#[test]
fn ci_runs_the_local_gate_commands() {
    let commands = run_commands(&read(".github/workflows/ci.yml"));
    assert!(commands.iter().any(|c| c == GATE_COMMAND));
    assert!(commands.iter().any(|c| c == SCAN_COMMAND));
    assert!(commands.iter().any(|c| c == FMT_COMMAND));
    assert!(commands.iter().any(|c| c == CLIPPY_COMMAND));
}

#[test]
fn ci_runs_the_cross_platform_gate_matrix() {
    let text = read(".github/workflows/ci.yml");
    let section = job_section(&text, "cross-platform");
    assert!(
        !section.is_empty(),
        "the workflow carries no cross-platform job"
    );
    let active: Vec<&str> = section
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    assert!(active.contains(&"fail-fast: false"));
    assert!(active.iter().any(|line| line.contains("macos-latest")));
    assert!(active.iter().any(|line| line.contains("windows-latest")));
    assert!(active.contains(&"runs-on: ${{ matrix.os }}"));
    let commands = run_commands_for_job(&text, "cross-platform");
    assert!(commands.iter().any(|c| c == GATE_COMMAND));
    assert!(commands.iter().any(|c| c == SCAN_COMMAND));
    assert!(commands.iter().any(|c| c == FMT_COMMAND));
    assert!(commands.iter().any(|c| c == CLIPPY_COMMAND));
}

#[test]
fn ci_runs_the_binding_smoke_test() {
    let text = read(".github/workflows/ci.yml");
    // Read the binding job alone, from its job line to the next
    // job line or the end of the file. The job builds the
    // binding and runs the smoke suite against it, and the
    // binding check is a required check, so a workflow that
    // dropped the smoke command, or disabled the job or one of
    // its steps with if false, would leave that check green
    // over a suite that never ran. A command that stands
    // commented out does not count as standing, and a step
    // condition is read wherever it stands in its step.
    // DB-S1 factors the job-boundary slice into `job_section` so
    // `test`, `cross-platform`, and `binding` share one parser.
    let section = job_section(&text, "binding");
    assert!(!section.is_empty(), "the workflow carries no binding job");
    for needle in [
        "maturin develop --manifest-path bindings/Cargo.toml",
        "python -m pytest tests/test_binding_smoke.py",
    ] {
        let stands = section.lines().any(|line| {
            let stripped = line.trim();
            !stripped.starts_with('#') && stripped.contains(needle)
        });
        assert!(stands, "the binding job lost {needle}");
    }
    let disabled = section.lines().any(|line| {
        let stripped = line.trim();
        let stripped = stripped.strip_prefix("- ").unwrap_or(stripped);
        stripped.strip_prefix("if:").is_some_and(|rest| {
            rest.trim()
                .trim_matches(|c| c == '\'' || c == '"')
                .to_lowercase()
                == "false"
        })
    });
    assert!(!disabled, "the binding job is disabled with if: false");
}

fn ci_hygiene_holds(text: &str) -> Result<(), String> {
    let active_lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    if !active_lines.contains(&"permissions:") || !active_lines.contains(&"contents: read") {
        return Err("missing read-only contents permission".to_string());
    }
    for line in &active_lines {
        if line.contains(": write") || line.contains("write-all") {
            return Err(format!("elevated permission line: {line}"));
        }
        let unbulleted = line.strip_prefix("- ").unwrap_or(line);
        if let Some(rest) = unbulleted.strip_prefix("if:") {
            let normalized = rest
                .trim()
                .trim_matches(|c| c == '\'' || c == '"')
                .to_lowercase();
            if normalized == "false" {
                return Err(format!("disabled job or step: {line}"));
            }
        }
    }
    let mut checkout_total = 0usize;
    for job in ["test", "cross-platform", "binding"] {
        let section = job_section(text, job);
        if section.is_empty() {
            return Err(format!("missing job {job}"));
        }
        let mut in_steps = false;
        let mut current_step: Vec<String> = Vec::new();
        let verify_step = |step: &[String], count: &mut usize| -> Result<(), String> {
            let is_checkout = step
                .iter()
                .any(|line| line.starts_with("uses:") && line.contains("actions/checkout@"));
            if is_checkout {
                *count += 1;
                if !step.iter().any(|line| line == "persist-credentials: false") {
                    return Err(format!(
                        "checkout step in job {job} lacks persist-credentials: false"
                    ));
                }
            }
            Ok(())
        };
        for raw_line in section.lines() {
            let stripped = raw_line.trim();
            if stripped.starts_with('#') || stripped.is_empty() {
                continue;
            }
            if stripped == "steps:" {
                in_steps = true;
                continue;
            }
            if in_steps && stripped.starts_with("- ") {
                verify_step(&current_step, &mut checkout_total)?;
                current_step.clear();
                current_step.push(stripped[2..].trim().to_string());
                continue;
            }
            if in_steps {
                current_step.push(stripped.to_string());
            }
        }
        verify_step(&current_step, &mut checkout_total)?;
    }
    if checkout_total < 3 {
        return Err(format!(
            "expected at least 3 checkouts, saw {checkout_total}"
        ));
    }
    Ok(())
}

#[test]
fn ci_carries_the_fleet_credential_hygiene() {
    let text = read(".github/workflows/ci.yml");
    // The workflow asks for no more than it uses. Its token
    // is limited to reading the repository, the checkout
    // leaves no credential behind for later steps, and every
    // toolchain step is pinned to a full commit SHA rather
    // than a moving branch name.
    // DB-S1 strengthens this pin from file-wide substring checks
    // to verify `persist-credentials: false` on every checkout
    // step across `test`, `cross-platform`, and `binding`, zero
    // `if: false` guards, and zero `: write` or `write-all` lines.
    if let Err(reason) = ci_hygiene_holds(&text) {
        panic!("CI hygiene failed, {reason}");
    }
    // Verify the hygiene check itself discriminates on one missing
    // persist-credentials setting, on a write permission, and on an
    // if: false guard in any job.
    let stripped_once = text.replacen("          persist-credentials: false\n", "", 1);
    assert!(ci_hygiene_holds(&stripped_once).is_err());
    let elevated = text.replace("contents: read", "contents: write");
    assert!(ci_hygiene_holds(&elevated).is_err());
    let write_all = text.replace("permissions:\n  contents: read", "permissions: write-all");
    assert!(ci_hygiene_holds(&write_all).is_err());
    let disabled_cross = text.replace("  cross-platform:\n", "  cross-platform:\n    if: false\n");
    assert!(ci_hygiene_holds(&disabled_cross).is_err());

    assert!(!text.contains("rust-toolchain@master"));
    let toolchain_lines: Vec<&str> = text
        .lines()
        .filter(|line| line.contains("dtolnay/rust-toolchain@"))
        .collect();
    assert_eq!(
        toolchain_lines.len(),
        3,
        "every job must pin dtolnay/rust-toolchain"
    );
    for toolchain_line in toolchain_lines {
        let pin = toolchain_line
            .split("dtolnay/rust-toolchain@")
            .nth(1)
            .unwrap()
            .trim();
        assert_eq!(pin.len(), 40, "{}", toolchain_line);
        assert!(pin
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    }
}

#[test]
fn ci_gate_parser_skips_a_step_disabled_with_if_false() {
    let ci_text = read(".github/workflows/ci.yml");
    let gate_step = "      - name: Run the local gate\n        run: cargo test";
    assert!(ci_text.contains(gate_step));
    for flag in ["false", "\"false\""] {
        let disabled = ci_text.replace(
            gate_step,
            &format!(
                "      - name: Run the local gate\n        if: {flag}\n        run: cargo test"
            ),
        );
        let commands = run_commands(&disabled);
        assert!(!commands.iter().any(|c| c == GATE_COMMAND));
        assert!(commands.iter().any(|c| c == SCAN_COMMAND));
    }
}

#[test]
fn ci_gate_parser_rejects_a_job_disabled_with_if_false() {
    let ci_text = read(".github/workflows/ci.yml");
    let job_head = "  test:\n    runs-on: ubuntu-latest";
    assert!(ci_text.contains(job_head));
    for flag in ["false", "\"false\""] {
        let disabled = ci_text.replace(
            job_head,
            &format!("  test:\n    if: {flag}\n    runs-on: ubuntu-latest"),
        );
        let commands = run_commands(&disabled);
        assert!(!commands.iter().any(|c| c == GATE_COMMAND));
        assert!(!commands.iter().any(|c| c == SCAN_COMMAND));
        assert!(!commands.iter().any(|c| c == FMT_COMMAND));
        assert!(!commands.iter().any(|c| c == CLIPPY_COMMAND));
    }
    let commands = run_commands(&ci_text);
    assert!(commands.iter().any(|c| c == GATE_COMMAND));
    assert!(commands.iter().any(|c| c == SCAN_COMMAND));
    assert!(commands.iter().any(|c| c == FMT_COMMAND));
    assert!(commands.iter().any(|c| c == CLIPPY_COMMAND));
}

struct ScanOutput {
    status: i32,
    stdout: String,
    stderr: String,
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

#[test]
fn codepoint_scan_reports_zero_for_the_repo() {
    let result = scan_tree(&root());
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
fn codepoint_scan_with_no_argument_scans_the_current_directory() {
    let dir = temp_dir("no-argument");
    std::fs::write(dir.join("note.md"), "a\u{2014}b").unwrap();
    let result = scan_current_dir(&dir);
    assert_eq!(result.status, 1);
    assert!(result.stdout.contains("em dash U+2014 count 1"));
}

const CANONICAL_ABOVE_MARKER_SHA256: &str =
    "bb3e2fc1790b13729da3495c3f0902b968577f4e45ceb27e48e90b489f36e1a7";

#[test]
fn agents_canonical_part_matches_pinned_digest() {
    let agents = std::fs::read(root().join("AGENTS.md")).unwrap();
    let marker: &[u8] = b"<!-- repo annex below this line -->";
    let position = agents
        .windows(marker.len())
        .position(|window| window == marker)
        .unwrap();
    let digest = common::sha256_hex(&agents[..position]);
    assert_eq!(digest, CANONICAL_ABOVE_MARKER_SHA256);
}

#[test]
fn readme_has_exactly_one_status_section() {
    let text = read("README.md");
    assert_eq!(text.matches("\n## Status\n").count(), 1);
}

#[test]
fn readme_carries_no_wrong_arc_label() {
    assert!(!read("README.md").contains("Fleet Operations"));
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
fn contract_obligations_carry_their_substance() {
    let text = read("CONTRACT.md");
    for phrase in [
        "present from the first commit",
        "every later change lands through a pull request",
        "no paid service",
        "cargo test plus the codepoint scan run through cargo",
        "0 of 100 until a full cycle",
        "no file carries a real trace or a real transcript or a real name",
        "SHA-256 digest of the part above the marked line",
    ] {
        assert!(text.contains(phrase), "{phrase}");
    }
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

#[test]
fn roadmap_and_manifest_versions_and_python_smoke_count_stay_in_sync() {
    let roadmap = read("ROADMAP.md");
    assert!(roadmap.starts_with("# Roadmap\n"));
    for slice in ["DB-S1", "DB-S2", "DB-S3", "DB-S4"] {
        assert!(roadmap.contains(slice), "ROADMAP.md missing {slice}");
    }

    let extract_version = |manifest: &str| -> String {
        manifest
            .lines()
            .find_map(|line| {
                let rest = line.trim().strip_prefix("version = \"")?;
                rest.strip_suffix('"').map(str::to_string)
            })
            .expect("manifest must declare version")
    };
    let root_version = extract_version(&read("Cargo.toml"));
    let bindings_version = extract_version(&read("bindings/Cargo.toml"));
    let pyproject_version = extract_version(&read("bindings/pyproject.toml"));
    assert_eq!(root_version, bindings_version);
    assert_eq!(root_version, pyproject_version);

    let py_smoke = read("tests/test_binding_smoke.py");
    let py_count = py_smoke
        .lines()
        .filter(|line| line.starts_with("def test_"))
        .count();
    let readme = read("README.md");
    let status = status_section(&readme);
    assert!(
        status.contains(&format!(
            "and the Python binding smoke test carries {py_count},"
        )),
        "README.md Python smoke test count drifted from {py_count}"
    );
}

#[cfg(unix)]
#[test]
fn codepoint_scan_with_an_unwritable_stdout_exits_2_without_a_panic() {
    // The scanner prints through a checked writer (DB-A24),
    // so a stream that refuses the write ends the scan at its
    // error exit 2 with no panic. /dev/full stands in for the
    // closed pipe of the finding's probe, because a pipe whose
    // reader closes early cannot be staged deterministically
    // from inside the suite, and /dev/full refuses the write
    // the same way. Where /dev/full does not stand the staged
    // failure cannot be produced and the pin does not run; on
    // the hosted gate, which is Linux, the device stands. On
    // the pre-fix binary the same drive dies with a panic
    // message at exit 101.
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
