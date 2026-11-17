use std::process::Command;

#[test]
fn missing_arguments_reports_usage() {
    let output = Command::new(env!("CARGO_BIN_EXE_rollup_client"))
        .output()
        .expect("run the rollup client");

    assert!(!output.status.success(), "missing arguments must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Usage:"), "expected usage, got: {stderr}");
    assert!(stderr.contains("<keypair.json> <recipient-pubkey> <lamports>"));
}
