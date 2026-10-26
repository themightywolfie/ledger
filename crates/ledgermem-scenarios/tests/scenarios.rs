//! Runs every YAML scenario under `scenarios/` against in-memory SQLite and a test clock.

use std::path::Path;

#[tokio::test]
async fn every_scenario_passes() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios");
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .expect("scenarios directory")
        .map(|entry| entry.expect("directory entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "yaml"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no scenarios found in {}", dir.display());

    let mut failures = Vec::new();
    for path in &paths {
        let yaml = std::fs::read_to_string(path).expect("readable scenario");
        if let Err(failure) = ledgermem_scenarios::run(&yaml).await {
            failures.push(format!("{}: {failure}", path.display()));
        }
    }
    assert!(
        failures.is_empty(),
        "scenario failures:\n{}",
        failures.join("\n")
    );
}
