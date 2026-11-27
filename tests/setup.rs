//! The setup pins, translated from the Python setup suite the repo
//! was seeded with, brought up to the current fleet standard. Each
//! test names the same fact its Python original named, with the
//! strengthenings the fleet folded in after dibs's Round 1 suite
//! was written, three of them landing with this port: the job level
//! if false pin, the wrong arc label pin, and the CI credential
//! hygiene pin. Two facts changed with the Rust conversion and the
//! change is stated where it lands. The CI gate commands are the
//! Rust gate commands of amended obligation S2, and the contract
//! substance pin asserts the amended S2 phrases. Slice DB-S4 amends
//! S2 and adds DB-25, moving codepoint_scan and its binary pins into
//! crates/codepoint_scan and adding workspace packaging, version
//! parity, PEP 561 stub, Docker, and release workflow pins here.

use std::path::PathBuf;

mod common;

const MARKER: &str = "<!-- repo annex below this line -->";
const GATE_COMMAND: &str = "cargo test";
// DB-S4 mechanical update (amended Obligation S2, DB-25): codepoint_scan
// now lives in the isolated workspace crate `crates/codepoint_scan` and
// runs through `-p codepoint_scan`.
const SCAN_COMMAND: &str = "cargo run -p codepoint_scan";
const FMT_COMMAND: &str = "cargo fmt --check";
const CLIPPY_COMMAND: &str = "cargo clippy --all-targets -- -D warnings";
const PACKAGE_COMMAND: &str = "cargo package --locked -p dibs-ai";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(name: &str) -> String {
    std::fs::read_to_string(root().join(name)).unwrap()
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
    let text = read(".github/workflows/ci.yml");
    let commands = run_commands(&text);
    assert!(commands.iter().any(|c| c == GATE_COMMAND));
    assert!(commands.iter().any(|c| c == SCAN_COMMAND));
    assert!(commands.iter().any(|c| c == FMT_COMMAND));
    assert!(commands.iter().any(|c| c == CLIPPY_COMMAND));
    assert!(commands.iter().any(|c| c == PACKAGE_COMMAND));

    let msrv_sec = job_section(&text, "msrv");
    assert!(!msrv_sec.is_empty(), "the workflow carries no msrv job");
    assert!(msrv_sec.contains("toolchain: \"1.89.0\""));
    let msrv_cmds = run_commands_for_job(&text, "msrv");
    assert!(msrv_cmds
        .iter()
        .any(|c| c == "cargo check --workspace --locked --all-targets"));
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
    let section = job_section(&text, "binding");
    assert!(!section.is_empty(), "the workflow carries no binding job");
    // DB-S4 extends the binding job pin under DB-25 to assert static
    // typechecking, wheel/sdist packaging, and Python 3.9/3.13 matrix
    // coverage alongside the smoke suite.
    for needle in [
        "maturin develop --manifest-path bindings/Cargo.toml",
        "python -m pytest tests/test_binding_smoke.py",
        "mypy --strict python/dibs tests/typecheck_dibs.py",
        "pyright python/dibs tests/typecheck_dibs.py",
        "maturin build --release --locked --out dist --manifest-path bindings/Cargo.toml",
        "maturin sdist --out dist --manifest-path bindings/Cargo.toml",
        "dibs/py.typed",
        "dibs/__init__.pyi",
        "python/dibs/py.typed",
        "python/dibs/__init__.pyi",
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

    let matrix_sec = job_section(&text, "binding-matrix");
    assert!(
        !matrix_sec.is_empty(),
        "the workflow carries no binding-matrix job"
    );
    assert!(matrix_sec.contains("runs-on: ubuntu-24.04"));
    assert!(matrix_sec.contains("python-version: [\"3.9\", \"3.13\"]"));
    assert!(matrix_sec.contains("maturin develop --manifest-path bindings/Cargo.toml"));
    assert!(matrix_sec.contains("python -m pytest tests/test_binding_smoke.py"));
}

fn verify_job_checkouts(
    text: &str,
    jobs: &[&str],
    min_checkouts: usize,
    required_ref: Option<&str>,
) -> Result<(), String> {
    let mut checkout_total = 0usize;
    for job in jobs {
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
                if let Some(expected_ref) = required_ref {
                    if !step.iter().any(|line| line == expected_ref) {
                        return Err(format!("checkout step in job {job} lacks {expected_ref}"));
                    }
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
    if checkout_total < min_checkouts {
        return Err(format!(
            "expected at least {min_checkouts} checkouts, saw {checkout_total}"
        ));
    }
    Ok(())
}

fn ci_hygiene_holds(text: &str) -> Result<(), String> {
    let Some((header, _)) = text.split_once("\njobs:\n") else {
        return Err("missing jobs: section".to_string());
    };
    let header_lines: Vec<&str> = header
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    if !header_lines.contains(&"permissions:") || !header_lines.contains(&"contents: read") {
        return Err("missing top-level read-only contents permission".to_string());
    }
    let active_lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
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
    verify_job_checkouts(
        text,
        &[
            "test",
            "cross-platform",
            "msrv",
            "binding",
            "binding-matrix",
        ],
        5,
        None,
    )
}

#[test]
fn ci_carries_the_fleet_credential_hygiene() {
    let text = read(".github/workflows/ci.yml");
    if let Err(reason) = ci_hygiene_holds(&text) {
        panic!("CI hygiene failed, {reason}");
    }
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
        5,
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
        "Obligation DB-25 covers",
    ] {
        assert!(text.contains(phrase), "{phrase}");
    }
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
    // DB-S4 mechanical update (DB-25): pyproject.toml now lives at the
    // repository root and bindings/pyproject.toml is removed; all four
    // public version declarations stand at 1.0.0.
    assert!(!root().join("bindings/pyproject.toml").exists());
    let root_version = extract_version(&read("Cargo.toml"));
    let bindings_version = extract_version(&read("bindings/Cargo.toml"));
    let pyproject_version = extract_version(&read("pyproject.toml"));
    let init_py = read("python/dibs/__init__.py");
    let py_version = init_py
        .lines()
        .find_map(|line| {
            let rest = line.trim().strip_prefix("__version__ = \"")?;
            rest.strip_suffix('"').map(str::to_string)
        })
        .expect("python/dibs/__init__.py must declare __version__");
    assert_eq!(root_version, "1.0.0");
    assert_eq!(root_version, bindings_version);
    assert_eq!(root_version, pyproject_version);
    assert_eq!(root_version, py_version);

    let scan_tests = read("crates/codepoint_scan/tests/scan.rs")
        .lines()
        .filter(|line| line.trim() == "#[test]")
        .count();
    assert_eq!(scan_tests, 15);
    let setup_tests = read("tests/setup.rs")
        .lines()
        .filter(|line| line.trim() == "#[test]")
        .count();
    assert_eq!(setup_tests, 21);

    let py_smoke = read("tests/test_binding_smoke.py");
    let py_count = py_smoke
        .lines()
        .filter(|line| line.starts_with("def test_"))
        .count();
    let readme = read("README.md");
    let status = status_section(&readme);
    let expected_census = format!(
        "The Rust workspace suite carries 204 tests across 189 root `dibs-ai` tests and \
         {scan_tests} `codepoint_scan` tests, running 203 on Unix (188 root and 15 scanner) \
         and 192 on Windows (180 root and 12 scanner), and the Python binding smoke test \
         carries {py_count},"
    );
    assert!(
        status.contains(&expected_census),
        "README.md test census drifted from {expected_census}"
    );
}

#[test]
fn workspace_and_package_manifests_satisfy_db25() {
    let root_cargo = read("Cargo.toml");
    for needle in [
        "members = [\".\", \"crates/codepoint_scan\", \"bindings\"]",
        "default-members = [\".\", \"crates/codepoint_scan\"]",
        "resolver = \"2\"",
        "name = \"dibs-ai\"",
        "version = \"1.0.0\"",
        "rust-version = \"1.89\"",
        "default-run = \"dibs\"",
        "readme = \"README.md\"",
        "repository = \"https://github.com/david-engelmann/dibs\"",
        "homepage = \"https://github.com/david-engelmann/dibs\"",
        "documentation = \"https://docs.rs/dibs-ai\"",
        "[lib]\nname = \"dibs\"\npath = \"src/lib.rs\"",
        "[[bin]]\nname = \"dibs\"\npath = \"src/bin/dibs.rs\"",
        "[profile.release]\nstrip = \"symbols\"",
        "[package.metadata.binstall]",
        "pkg-url = \"{ repo }/releases/download/v{ version }/dibs-v{ version }-{ target }{ archive-suffix }\"",
        "bin-dir = \"{ bin }{ binary-ext }\"",
        "pkg-fmt = \"tgz\"",
        "[package.metadata.binstall.overrides.x86_64-pc-windows-msvc]",
        "pkg-fmt = \"zip\"",
    ] {
        assert!(root_cargo.contains(needle), "Cargo.toml missing {needle}");
    }
    assert_eq!(
        root_cargo.matches("[[bin]]").count(),
        1,
        "root Cargo.toml must declare only the dibs binary"
    );
    assert!(!root_cargo.contains("src/bin/codepoint_scan.rs"));
    assert!(!root().join("src/bin/codepoint_scan.rs").exists());
    for excluded in [
        "\"AGENTS.md\"",
        "\"CONTRACT.md\"",
        "\"ROADMAP.md\"",
        "\"CHANGELOG.md\"",
        "\"Dockerfile\"",
        "\".dockerignore\"",
        "\"pyproject.toml\"",
        "\"bindings/**\"",
        "\"crates/**\"",
        "\"python/**\"",
        "\"tests/**\"",
        "\".github/**\"",
    ] {
        assert!(
            root_cargo.contains(excluded),
            "Cargo.toml exclude missing {excluded}"
        );
    }

    let gitignore = read(".gitignore");
    for entry in [".mypy_cache/", ".pytest_cache/", ".venv-binding/"] {
        assert!(
            gitignore.lines().any(|line| line.trim() == entry),
            ".gitignore missing {entry}"
        );
    }

    let scan_cargo = read("crates/codepoint_scan/Cargo.toml");
    assert!(scan_cargo.contains("name = \"codepoint_scan\""));
    assert!(scan_cargo.contains("publish = false"));
    assert!(root().join("crates/codepoint_scan/src/main.rs").is_file());
    assert!(root().join("crates/codepoint_scan/tests/scan.rs").is_file());

    let bind_cargo = read("bindings/Cargo.toml");
    assert!(bind_cargo.contains("publish = false"));
    assert!(bind_cargo.contains("rust-version = \"1.89\""));
    assert!(bind_cargo.contains("[lib]\nname = \"_dibs\"\ncrate-type = [\"cdylib\"]"));
    assert!(bind_cargo.contains("dibs-core = { package = \"dibs-ai\", path = \"..\" }"));

    let pyproject = read("pyproject.toml");
    for needle in [
        "name = \"dibs-ai\"",
        "version = \"1.0.0\"",
        "requires-python = \">=3.9\"",
        "\"Typing :: Typed\"",
        "manifest-path = \"bindings/Cargo.toml\"",
        "module-name = \"dibs._dibs\"",
        "python-source = \"python\"",
        "features = [\"extension-module\"]",
        "include = [{ path = \"LICENSE\", format = [\"sdist\", \"wheel\"] }]",
    ] {
        assert!(
            pyproject.contains(needle),
            "pyproject.toml missing {needle}"
        );
    }
}

#[test]
fn pep561_type_stubs_cover_public_python_surface() {
    let py_typed = root().join("python/dibs/py.typed");
    assert!(py_typed.is_file(), "python/dibs/py.typed missing");
    assert_eq!(std::fs::read(&py_typed).unwrap().len(), 0);
    assert!(root().join("tests/typecheck_dibs.py").is_file());

    let pyi = read("python/dibs/__init__.pyi");
    let init_py = read("python/dibs/__init__.py");
    assert!(pyi.contains("StrPath: TypeAlias = str | os.PathLike[str]"));
    assert!(init_py.contains("StrPath = Union[str, os.PathLike[str]]"));
    assert!(pyi.contains("__version__: str"));

    for fn_name in [
        "offer",
        "offer_many",
        "claim",
        "claim_next",
        "renew",
        "release",
        "complete",
        "reset",
        "board",
        "ready",
        "show",
        "history",
        "compact",
    ] {
        assert!(
            pyi.contains(&format!("def {fn_name}(")),
            "__init__.pyi missing def {fn_name}"
        );
    }
    for td_name in [
        "OfferSpecDict",
        "ClaimGrantedDict",
        "RenewedDict",
        "BoardRowDict",
        "ItemDetailDict",
        "EventDict",
        "CompactSummaryDict",
    ] {
        assert!(
            pyi.contains(&format!("class {td_name}(")),
            "__init__.pyi missing TypedDict {td_name}"
        );
        assert!(
            init_py.contains(&format!("class {td_name}(")),
            "__init__.py missing TypedDict {td_name}"
        );
    }
    for exc_name in [
        "DibsError",
        "InvalidNameError",
        "InvalidTtlError",
        "InvalidRequestError",
        "NotFoundError",
        "RefusalError",
        "CorruptError",
        "FailureError",
    ] {
        assert!(
            pyi.contains(&format!("class {exc_name}(")),
            "__init__.pyi missing exception {exc_name}"
        );
    }
}

#[test]
fn dockerfile_and_changelog_satisfy_db25() {
    let dockerfile = read("Dockerfile");
    for needle in [
        "FROM rust:1.91.1-bookworm AS builder",
        "cargo build --release --locked --bin dibs",
        "strip /src/target/release/dibs",
        "FROM debian:bookworm-slim",
        "ca-certificates",
        "useradd --uid 65532",
        "USER 65532:65532",
        "WORKDIR /work",
        "ENTRYPOINT [\"dibs\"]",
    ] {
        assert!(dockerfile.contains(needle), "Dockerfile missing {needle}");
    }
    let dockerignore = read(".dockerignore");
    for entry in [
        "target",
        ".git",
        ".venv*",
        "dist",
        "__pycache__",
        ".pytest_cache",
        ".mypy_cache",
    ] {
        assert!(
            dockerignore.lines().any(|line| line.trim() == entry),
            ".dockerignore missing {entry}"
        );
    }
    let changelog = read("CHANGELOG.md");
    assert!(changelog.starts_with("# Changelog\n"));
    assert!(changelog.contains("1.0.0"));
}

fn release_workflow_holds(text: &str) -> Result<(), String> {
    let Some((header, _)) = text.split_once("\njobs:\n") else {
        return Err("missing jobs: section".to_string());
    };
    let header_lines: Vec<&str> = header
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    if !header_lines.contains(&"permissions:") || !header_lines.contains(&"contents: read") {
        return Err("missing top-level read-only contents permission".to_string());
    }
    if !header_lines.contains(&"concurrency:")
        || !header_lines.contains(&"group: release-${{ inputs.tag || github.ref_name }}")
        || !header_lines.contains(&"cancel-in-progress: false")
    {
        return Err("missing tag-scoped non-cancelling concurrency group".to_string());
    }
    for line in &header_lines {
        if line.contains(": write") || line.contains("write-all") {
            return Err(format!("elevated top-level permission line: {line}"));
        }
    }
    for req_trigger in ["tags:", "- \"v*\"", "workflow_dispatch:", "tag:"] {
        if !header_lines.contains(&req_trigger) {
            return Err(format!("missing trigger element {req_trigger}"));
        }
    }

    let active_lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    let mut pinned_third_party = 0usize;
    for line in &active_lines {
        if line.contains("write-all") {
            return Err(format!("write-all permission forbidden: {line}"));
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
        if let Some(raw_uses) = unbulleted.strip_prefix("uses:") {
            let spec = raw_uses.split('#').next().unwrap_or("").trim();
            if !spec.starts_with("actions/") {
                let Some((_, pin)) = spec.split_once('@') else {
                    return Err(format!("unpinned third-party action: {line}"));
                };
                if pin.len() != 40
                    || !pin
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    return Err(format!(
                        "third-party action must pin 40-char commit SHA: {line}"
                    ));
                }
                pinned_third_party += 1;
            }
        }
    }
    if pinned_third_party < 12 {
        return Err(format!(
            "expected at least 12 SHA-pinned third-party uses, saw {pinned_third_party}"
        ));
    }

    let required_jobs = [
        "verify",
        "binaries",
        "wheels",
        "sdist",
        "publish-docker",
        "github-release",
        "publish-crates",
        "publish-pypi",
    ];
    for job in &required_jobs {
        if job_section(text, job).is_empty() {
            return Err(format!("missing job {job}"));
        }
    }
    for ro_job in ["verify", "binaries", "wheels", "sdist", "publish-crates"] {
        let sec = job_section(text, ro_job);
        let ro_lines: Vec<&str> = sec
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .collect();
        if !ro_lines.contains(&"permissions:") || !ro_lines.contains(&"contents: read") {
            return Err(format!("read-only job {ro_job} missing contents: read"));
        }
        for line in &ro_lines {
            if line.contains(": write") {
                return Err(format!(
                    "read-only job {ro_job} carries write permission: {line}"
                ));
            }
        }
    }

    let verify_sec = job_section(text, "verify");
    if !verify_sec.contains("outputs:")
        || !verify_sec.contains("commit_sha: ${{ steps.verify_meta.outputs.commit_sha }}")
    {
        return Err("verify job must export steps.verify_meta.outputs.commit_sha".to_string());
    }
    let verify_ref = "ref: ${{ inputs.tag && format('refs/tags/{0}', inputs.tag) || github.ref }}";
    verify_job_checkouts(text, &["verify"], 1, Some(verify_ref))?;

    // Verify all 7 downstream jobs check out the tag ref (avoiding CodeQL workflow_dispatch
    // cache-poisoning on untrusted job outputs) and immediately assert checked-out HEAD
    // equals the immutable commit_sha emitted by verify.
    let downstream_jobs = [
        "binaries",
        "wheels",
        "sdist",
        "publish-docker",
        "github-release",
        "publish-crates",
        "publish-pypi",
    ];
    verify_job_checkouts(text, &downstream_jobs, 5, Some(verify_ref))?;
    for checkout_job in [
        "binaries",
        "wheels",
        "sdist",
        "publish-docker",
        "publish-crates",
    ] {
        let sec = job_section(text, checkout_job);
        if !sec.contains("EXPECTED_SHA: ${{ needs.verify.outputs.commit_sha }}")
            || !sec.contains("test \"$(git rev-parse --verify HEAD)\" = \"$EXPECTED_SHA\"")
        {
            return Err(format!(
                "downstream job {checkout_job} must verify HEAD against needs.verify.outputs.commit_sha"
            ));
        }
    }

    let bin_sec = job_section(text, "binaries");
    for target in [
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
        "x86_64-pc-windows-msvc",
    ] {
        if !bin_sec.contains(target) {
            return Err(format!("binaries matrix missing {target}"));
        }
    }
    if !bin_sec.contains("newline=\"\\n\"") {
        return Err("binaries .sha256 write must enforce LF newline".to_string());
    }

    let wheels_sec = job_section(text, "wheels");
    let wheel_targets = wheels_sec
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("target: ") && !line.contains("${{"))
        .count();
    if wheel_targets != 5
        || !wheels_sec.contains("target: x86_64")
        || !wheels_sec.contains("target: aarch64")
        || !wheels_sec.contains("target: x64")
    {
        return Err(format!(
            "expected 5 wheel matrix targets, saw {wheel_targets}"
        ));
    }

    let sdist_sec = job_section(text, "sdist");
    if !sdist_sec.contains("python/dibs/py.typed")
        || !sdist_sec.contains("python/dibs/__init__.pyi")
    {
        return Err("sdist job must verify py.typed and __init__.pyi in archive".to_string());
    }

    let docker_sec = job_section(text, "publish-docker");
    if !docker_sec.contains("needs: [verify]") {
        return Err("publish-docker must depend only on [verify]".to_string());
    }
    if docker_sec.contains("publish-crates") || docker_sec.contains("publish-pypi") {
        return Err("publish-docker must not depend on crates or pypi".to_string());
    }
    let gh_sec = job_section(text, "github-release");
    if !gh_sec.contains("needs: [verify, binaries, wheels, sdist]") {
        return Err(
            "github-release must depend only on [verify, binaries, wheels, sdist]".to_string(),
        );
    }
    if gh_sec.contains("publish-crates") || gh_sec.contains("publish-pypi") {
        return Err("github-release must not depend on crates or pypi".to_string());
    }
    if !gh_sec.contains("SHA256SUMS.txt") || !gh_sec.contains("newline=\"\\n\"") {
        return Err("github-release must generate LF-terminated SHA256SUMS.txt".to_string());
    }
    let pypi_sec = job_section(text, "publish-pypi");
    if !pypi_sec.contains("skip-existing: true") {
        return Err("publish-pypi must enable skip-existing: true".to_string());
    }
    Ok(())
}

#[test]
fn release_workflow_carries_db25_jobs_and_credential_hygiene() {
    let text = read(".github/workflows/release.yml");
    if let Err(reason) = release_workflow_holds(&text) {
        panic!("release.yml failed DB-25 check, {reason}");
    }
    // Prove the validator discriminates against top-level permission deletion or elevation,
    // read-only job elevation, missing persist-credentials, job-graph coupling, unpinned
    // third-party actions, and dropped release targets.
    let no_top_perms = text.replacen("permissions:\n  contents: read\n\n", "", 1);
    assert!(release_workflow_holds(&no_top_perms).is_err());
    let no_concurrency = text.replacen(
        "concurrency:\n  group: release-${{ inputs.tag || github.ref_name }}\n  cancel-in-progress: false\n\n",
        "",
        1,
    );
    assert!(release_workflow_holds(&no_concurrency).is_err());
    let cancelling_concurrency =
        text.replacen("cancel-in-progress: false", "cancel-in-progress: true", 1);
    assert!(release_workflow_holds(&cancelling_concurrency).is_err());
    let unpinned_checkout_ref = text.replacen(
        "          ref: ${{ inputs.tag && format('refs/tags/{0}', inputs.tag) || github.ref }}\n",
        "",
        1,
    );
    assert!(release_workflow_holds(&unpinned_checkout_ref).is_err());
    let missing_sha_guard = text.replacen(
        "          EXPECTED_SHA: ${{ needs.verify.outputs.commit_sha }}\n",
        "",
        1,
    );
    assert!(release_workflow_holds(&missing_sha_guard).is_err());
    let write_top_perms = text.replacen(
        "permissions:\n  contents: read",
        "permissions:\n  contents: write",
        1,
    );
    assert!(release_workflow_holds(&write_top_perms).is_err());
    let elevated_verify = text.replacen(
        "  verify:\n    runs-on: ubuntu-latest\n    permissions:\n      contents: read",
        "  verify:\n    runs-on: ubuntu-latest\n    permissions:\n      contents: write",
        1,
    );
    assert!(release_workflow_holds(&elevated_verify).is_err());
    let missing_persist = text.replacen("          persist-credentials: false\n", "", 1);
    assert!(release_workflow_holds(&missing_persist).is_err());
    let coupled_gh = text.replace(
        "  github-release:\n    needs: [verify, binaries, wheels, sdist]",
        "  github-release:\n    needs: [verify, binaries, wheels, sdist, publish-crates]",
    );
    assert!(release_workflow_holds(&coupled_gh).is_err());
    let unpinned_rust = text.replacen(
        "dtolnay/rust-toolchain@683b588c487a69d01f6c6ddf40f036ff682cc2c8",
        "dtolnay/rust-toolchain@stable",
        1,
    );
    assert!(release_workflow_holds(&unpinned_rust).is_err());
    let unpinned_maturin = text.replacen(
        "PyO3/maturin-action@e83996d129638aa358a18fbd1dfb82f0b0fb5d3b",
        "PyO3/maturin-action@v1",
        1,
    );
    assert!(release_workflow_holds(&unpinned_maturin).is_err());
    let dropped_intel_mac = text.replace("x86_64-apple-darwin", "removed-target");
    assert!(release_workflow_holds(&dropped_intel_mac).is_err());

    assert!(job_section(&text, "verify").contains("refs/tags/{target_tag}^{{commit}}"));
    assert!(job_section(&text, "publish-docker").contains("ghcr.io/david-engelmann/dibs"));
    assert!(job_section(&text, "publish-docker").contains("linux/amd64,linux/arm64"));
    assert!(job_section(&text, "publish-crates").contains("cargo publish --locked -p dibs-ai"));
    assert!(job_section(&text, "publish-pypi")
        .contains("pypa/gh-action-pypi-publish@7245906fb61fa33a13554f9d5fb856fe61758086"));
}
