//! Running the project's tests, and reading the result.
//!
//! A diff tells you what an agent changed; the test run tells you whether to
//! trust it. NOIDA finds the command the project already uses rather than
//! inventing one, runs it in the agent's own worktree so parallel agents do not
//! fight over a checkout, and parses just enough of the output to say
//! "312 passed, 1 failed" and to point at the failures.
//!
//! Output is handed back verbatim as well, so the reference resolver can turn
//! the `file:line` in a stack trace into somewhere you can jump to.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;

use crate::events::Bg;

/// How a project runs its tests.
#[derive(Clone, Debug, PartialEq)]
pub struct Runner {
    /// Program and arguments, e.g. `["cargo", "test"]`.
    pub command: Vec<String>,
    /// What to call it in the UI.
    pub label: String,
}

impl Runner {
    fn new(command: &[&str]) -> Self {
        Runner {
            command: command.iter().map(|s| s.to_string()).collect(),
            label: command.join(" "),
        }
    }

    /// Work out how to test the project at `root`.
    ///
    /// Manifests are checked in the order a developer would: an explicit
    /// setting wins, then the language manifest that is actually present.
    /// Returns `None` rather than guessing when nothing matches.
    pub fn detect(root: &Path, configured: Option<&str>) -> Option<Runner> {
        if let Some(cmd) = configured.map(str::trim).filter(|c| !c.is_empty()) {
            let parts: Vec<&str> = cmd.split_whitespace().collect();
            return Some(Runner { command: parts.iter().map(|s| s.to_string()).collect(), label: cmd.to_string() });
        }
        let has = |name: &str| root.join(name).exists();

        if has("Cargo.toml") {
            return Some(Runner::new(&["cargo", "test"]));
        }
        if has("go.mod") {
            return Some(Runner::new(&["go", "test", "./..."]));
        }
        if has("package.json") {
            // Only claim a JS project tests if it says so; `npm test` on a
            // package without a test script just fails confusingly.
            let manifest = std::fs::read_to_string(root.join("package.json")).unwrap_or_default();
            if package_has_test_script(&manifest) {
                let pm = js_package_manager(root);
                return Some(Runner::new(&[pm, "test"]));
            }
        }
        if has("pytest.ini") || has("tox.ini") || has("pyproject.toml") || has("setup.cfg") {
            return Some(Runner::new(&["pytest"]));
        }
        None
    }
}

/// True when package.json declares a `test` script that is not the npm default
/// placeholder ("no test specified").
fn package_has_test_script(manifest: &str) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(manifest) else { return false };
    let Some(test) = v["scripts"]["test"].as_str() else { return false };
    !test.contains("no test specified")
}

fn js_package_manager(root: &Path) -> &'static str {
    if root.join("pnpm-lock.yaml").exists() {
        "pnpm"
    } else if root.join("yarn.lock").exists() {
        "yarn"
    } else if root.join("bun.lockb").exists() {
        "bun"
    } else {
        "npm"
    }
}

/// What a finished run tells us.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    pub passed: usize,
    pub failed: usize,
    /// Names of failing tests, in the order the runner printed them.
    pub failures: Vec<String>,
    /// The runner's exit status, which decides pass/fail when counts are absent.
    pub ok: bool,
}

impl Outcome {
    /// One line for the status bar or a banner.
    pub fn summary(&self) -> String {
        match (self.passed, self.failed) {
            (0, 0) if self.ok => "tests passed".to_string(),
            (0, 0) => "tests failed".to_string(),
            (p, 0) => format!("{p} passed"),
            (0, f) => format!("{f} failed"),
            (p, f) => format!("{p} passed, {f} failed"),
        }
    }
}

/// Read counts and failing test names out of a run's output.
///
/// Every runner reports differently and none of them promise a stable format,
/// so this reads the shapes the common ones actually print and otherwise falls
/// back to the exit status. It is deliberately forgiving: a missed count shows
/// up as a plain pass/fail, never as a wrong number.
pub fn parse(output: &str, ok: bool) -> Outcome {
    let mut out = Outcome { ok, ..Default::default() };

    for line in output.lines() {
        let line = line.trim();

        // cargo: "test result: ok. 67 passed; 0 failed; 2 ignored; ..."
        if let Some(rest) = line.strip_prefix("test result:") {
            for part in rest.split(';') {
                let part = part.trim().trim_start_matches("ok.").trim_start_matches("FAILED.").trim();
                if let Some(n) = part.strip_suffix(" passed").and_then(|n| n.trim().parse::<usize>().ok()) {
                    out.passed += n;
                }
                if let Some(n) = part.strip_suffix(" failed").and_then(|n| n.trim().parse::<usize>().ok()) {
                    out.failed += n;
                }
            }
        }
        // cargo and go name failures one per line.
        if let Some(name) = line.strip_prefix("---- ").and_then(|l| l.strip_suffix(" stdout ----")) {
            out.failures.push(name.to_string());
        } else if let Some(name) = line.strip_prefix("--- FAIL: ") {
            out.failures.push(name.split_whitespace().next().unwrap_or(name).to_string());
        } else if let Some(name) = line.strip_prefix("FAIL ") {
            out.failures.push(name.split_whitespace().next().unwrap_or(name).to_string());
        }
        // jest/vitest: "Tests:  1 failed, 11 passed, 12 total"
        if line.starts_with("Tests:") {
            for part in line.trim_start_matches("Tests:").split(',') {
                let mut words = part.split_whitespace();
                let (Some(n), Some(what)) = (words.next(), words.next()) else { continue };
                let Ok(n) = n.parse::<usize>() else { continue };
                match what {
                    "passed" => out.passed += n,
                    "failed" => out.failed += n,
                    _ => {}
                }
            }
        }
        // pytest: "=== 2 failed, 10 passed in 0.4s ==="
        if line.starts_with('=') && (line.contains(" passed") || line.contains(" failed")) {
            let words: Vec<&str> = line.trim_matches('=').split_whitespace().collect();
            for pair in words.windows(2) {
                let Ok(n) = pair[0].parse::<usize>() else { continue };
                match pair[1].trim_end_matches(',') {
                    "passed" => out.passed += n,
                    "failed" | "error" | "errors" => out.failed += n,
                    _ => {}
                }
            }
        }
    }

    // A runner that said nothing countable still said something by exiting.
    if out.passed == 0 && out.failed == 0 && !ok {
        out.failed = out.failures.len();
    }
    out
}

/// Run the tests in `dir`, streaming output back as it arrives.
///
/// `id` identifies the run so late output from a cancelled one can be ignored.
pub fn run(id: u64, runner: Runner, dir: PathBuf, tx: Sender<Bg>) {
    std::thread::spawn(move || {
        let mut cmd = Command::new(&runner.command[0]);
        cmd.args(&runner.command[1..]).current_dir(&dir);
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        // Test runners colour their output for a terminal; we render it as
        // plain text, so ask them not to.
        cmd.env("NO_COLOR", "1").env("CARGO_TERM_COLOR", "never");

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let text = format!("could not run `{}`: {e}", runner.label);
                let _ = tx.send(Bg::TestDone(id, Outcome { ok: false, ..Default::default() }, text));
                return;
            }
        };
        let output = match child.wait_with_output() {
            Ok(o) => o,
            Err(e) => {
                let _ = tx.send(Bg::TestDone(id, Outcome { ok: false, ..Default::default() }, e.to_string()));
                return;
            }
        };
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&output.stderr));
        let outcome = parse(&text, output.status.success());
        let _ = tx.send(Bg::TestDone(id, outcome, text));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        // Keep the name boring: cargo puts this path on $LD_LIBRARY_PATH, which
        // cannot carry colons or spaces.
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!("noida-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn detects_the_command_a_project_already_uses() {
        let d = dir();
        assert_eq!(Runner::detect(&d, None), None, "nothing to go on");

        // An explicit setting always wins.
        let configured = Runner::detect(&d, Some("just test")).unwrap();
        assert_eq!(configured.command, vec!["just", "test"]);
        assert_eq!(Runner::detect(&d, Some("   ")), None, "blank setting is not a command");

        std::fs::write(d.join("Cargo.toml"), "[package]\nname='x'\n").unwrap();
        assert_eq!(Runner::detect(&d, None).unwrap().command, vec!["cargo", "test"]);

        // A package.json without a real test script is not a test runner.
        let js = dir();
        std::fs::write(js.join("package.json"), r#"{"scripts":{"test":"echo \"Error: no test specified\" && exit 1"}}"#).unwrap();
        assert_eq!(Runner::detect(&js, None), None);

        std::fs::write(js.join("package.json"), r#"{"scripts":{"test":"vitest run"}}"#).unwrap();
        assert_eq!(Runner::detect(&js, None).unwrap().command, vec!["npm", "test"]);
        std::fs::write(js.join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(Runner::detect(&js, None).unwrap().command, vec!["pnpm", "test"]);

        let py = dir();
        std::fs::write(py.join("pyproject.toml"), "").unwrap();
        assert_eq!(Runner::detect(&py, None).unwrap().command, vec!["pytest"]);

        std::fs::remove_dir_all(&d).ok();
        std::fs::remove_dir_all(&js).ok();
        std::fs::remove_dir_all(&py).ok();
    }

    #[test]
    fn reads_counts_from_each_runner() {
        let cargo = parse("test result: ok. 67 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out;", true);
        assert_eq!((cargo.passed, cargo.failed), (67, 0));
        assert_eq!(cargo.summary(), "67 passed");

        let cargo_fail = parse("---- refs::tests::partial_paths stdout ----\ntest result: FAILED. 62 passed; 4 failed; 2 ignored;", false);
        assert_eq!((cargo_fail.passed, cargo_fail.failed), (62, 4));
        assert_eq!(cargo_fail.failures, vec!["refs::tests::partial_paths"]);
        assert_eq!(cargo_fail.summary(), "62 passed, 4 failed");

        let jest = parse("Tests:       1 failed, 11 passed, 12 total", false);
        assert_eq!((jest.passed, jest.failed), (11, 1));

        let pytest = parse("=========== 2 failed, 10 passed in 0.42s ============", false);
        assert_eq!((pytest.passed, pytest.failed), (10, 2));

        let go = parse("--- FAIL: TestCheckout (0.00s)\nFAIL\texample/checkout\t0.2s", false);
        assert!(go.failures.contains(&"TestCheckout".to_string()));

        // Nothing countable: fall back to the exit status rather than inventing.
        assert_eq!(parse("", true).summary(), "tests passed");
        assert_eq!(parse("boom", false).summary(), "tests failed");
    }

    #[test]
    fn runs_a_real_command_end_to_end() {
        use std::sync::mpsc::channel;
        let d = dir();
        // A tiny cargo project whose test fails, so both paths are exercised.
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::write(d.join("Cargo.toml"), "[package]\nname=\"probe\"\nversion=\"0.0.0\"\nedition=\"2021\"\n").unwrap();
        std::fs::write(d.join("src/lib.rs"), "#[test]\nfn ok_one() {}\n#[test]\nfn fails() { assert_eq!(1, 2); }\n").unwrap();

        let runner = Runner::detect(&d, None).expect("cargo project");
        let (tx, rx) = channel();
        run(1, runner, d.clone(), tx);
        let Bg::TestDone(id, outcome, output) = rx.recv_timeout(std::time::Duration::from_secs(180)).expect("a result") else {
            panic!("wrong event")
        };
        assert_eq!(id, 1);
        assert!(!outcome.ok, "a failing test must not report ok");
        assert_eq!((outcome.passed, outcome.failed), (1, 1), "output was:\n{output}");
        assert_eq!(outcome.failures, vec!["fails"]);
        assert_eq!(outcome.summary(), "1 passed, 1 failed");
        std::fs::remove_dir_all(&d).ok();
    }
}
