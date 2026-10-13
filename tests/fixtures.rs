//! Fixture tests: run the release-independent binary on each `tests/fixtures/*` project and compare
//! the flat report with `expected.txt`.

use std::path::Path;
use std::process::Command;

fn run_fixture(name: &str) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
    let out = Command::new(env!("CARGO_BIN_EXE_tidy"))
        .arg(&dir)
        .args(["--reporter", "flat", "--no-exit-code"])
        .output()
        .expect("run tidy");
    assert!(out.status.success(), "tidy failed: {}", String::from_utf8_lossy(&out.stderr));
    let json: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).expect("flat json");
    let actual: Vec<String> = json
        .iter()
        .map(|i| format!("{} {}:{}:{} {}", i["type"].as_str().unwrap(), i["file"].as_str().unwrap(), i["line"], i["col"], i["name"].as_str().unwrap()).trim_end().to_string())
        .collect();
    let expected: Vec<String> = std::fs::read_to_string(dir.join("expected.txt")).unwrap().lines().map(|l| l.to_string()).collect();
    if actual != expected {
        let a = actual.join("\n");
        let e = expected.join("\n");
        panic!("fixture {name} differs\n--- expected ---\n{e}\n--- actual ---\n{a}\n");
    }
}

#[test]
fn basic() {
    run_fixture("basic");
}
