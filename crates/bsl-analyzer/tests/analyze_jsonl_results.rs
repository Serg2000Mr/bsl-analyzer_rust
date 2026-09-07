use std::{fs, path::Path, process::Command};

use serde_json::Value;
use tempfile::TempDir;

fn run_jsonl(source_dir: &Path) -> Vec<Value> {
    let output = Command::new(env!("CARGO_BIN_EXE_bsl-analyzer-app"))
        .args(["analyze", "-s"])
        .arg(source_dir)
        .args(["--format", "jsonl", "--only-diagnostic", "SelfAssign", "--workers", "1", "-q"])
        .env("BSL_SALSA_CHUNK", "1")
        .output()
        .expect("run bsl-analyzer");

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8(output.stdout)
        .expect("UTF-8 JSONL")
        .lines()
        .map(|line| serde_json::from_str(line).expect("one JSON event per line"))
        .collect()
}

#[test]
fn jsonl_keeps_findings_clean_files_and_totals_across_chunks() {
    let temp = TempDir::new().expect("tempdir");
    fs::write(
        temp.path().join("Finding.bsl"),
        "Процедура Тест()\n    А = А;\nКонецПроцедуры\n",
    )
    .expect("finding fixture");
    fs::write(
        temp.path().join("Clean.bsl"),
        "Процедура Тест()\n    А = 1;\nКонецПроцедуры\n",
    )
    .expect("clean fixture");

    let events = run_jsonl(temp.path());
    assert_eq!(events.len(), 4);
    assert_eq!(events[0]["type"], "start");
    assert_eq!(events[0]["total_files"], 2);

    let mut findings = 0;
    let mut clean_files = 0;
    let mut paths = std::collections::BTreeSet::new();
    for event in &events[1..3] {
        assert_eq!(event["type"], "file");
        assert!(event.get("error").is_none());
        assert!(event.get("metrics").is_none());
        let path = Path::new(event["path"].as_str().expect("file path"));
        let name = path.file_name().expect("file name").to_str().expect("UTF-8 file name");
        assert!(paths.insert(name.to_owned()), "one event per file");
        let diagnostics = event["diagnostics"].as_array().expect("diagnostics array");
        match name {
            "Finding.bsl" => {
                assert!(!diagnostics.is_empty(), "fixture must produce a finding");
                for diagnostic in diagnostics {
                    assert_eq!(diagnostic["code"], "SelfAssign");
                    assert!(diagnostic["message"].as_str().is_some_and(|text| !text.is_empty()));
                }
                findings += diagnostics.len();
            }
            "Clean.bsl" => {
                assert!(diagnostics.is_empty());
                clean_files += 1;
            }
            other => panic!("unexpected file: {other}"),
        }
    }

    assert_eq!(clean_files, 1);
    assert_eq!(paths.len(), 2);
    let done = events.last().expect("done event");
    assert_eq!(done["type"], "done");
    assert_eq!(done["total_files"], 2);
    assert_eq!(done["total_diagnostics"].as_u64(), Some(findings as u64));
    assert_eq!(done["failed_files"], 0);
}

#[test]
fn jsonl_empty_project_still_emits_start_and_done() {
    let temp = TempDir::new().expect("tempdir");
    let events = run_jsonl(temp.path());

    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["type"], "start");
    assert_eq!(events[0]["total_files"], 0);
    assert_eq!(events[1]["type"], "done");
    assert_eq!(events[1]["total_files"], 0);
    assert_eq!(events[1]["total_diagnostics"], 0);
    assert_eq!(events[1]["failed_files"], 0);
}
