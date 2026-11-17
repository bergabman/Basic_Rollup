//! Project tasks, run as `cargo xtask <task>`.

use std::{
    env, fs,
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{bail, ensure, Context, Result};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use solana_keypair::{read_keypair_file, write_keypair_file, Keypair};
use solana_signer::Signer;

const USAGE: &str = "\
Usage: cargo xtask <task>

Tasks:
  demo     Create the demo keys and genesis in demo/ and print how to run the rollup
  verify   Format, lint and test the workspace, then run the smoke test
  smoke    Run the real server and client on loopback and check the resulting balances
  mainnet  Compare the rollup's Mainnet snapshot with the live cluster (needs network)
  live     Pull an account from ROLLUP_TEST_CLUSTER_URL (default devnet; needs network)";

const GENESIS_LAMPORTS: u64 = 10_000_000_000;
const TRANSFER_LAMPORTS: u64 = 100_000_000;
const TRANSFERS: u64 = 2;
const FEE_LAMPORTS: u64 = 5_000;

fn main() -> Result<()> {
    match env::args().nth(1).as_deref() {
        Some("demo") => demo(),
        Some("verify") => verify(),
        Some("smoke") => smoke(),
        Some("mainnet") => ignored_test("mainnet_snapshot_is_current"),
        Some("live") => ignored_test("live_cluster_account_matches_the_rollup"),
        _ => bail!("{USAGE}"),
    }
}

fn verify() -> Result<()> {
    cargo(&["fmt", "--all"])?;
    cargo(&[
        "clippy",
        "--workspace",
        "--all-targets",
        "--locked",
        "--",
        "-D",
        "warnings",
    ])?;
    cargo(&["test", "--workspace", "--locked"])?;

    smoke()
}

/// Creates the demo keys and genesis, then prints how to run the rollup by hand.
fn demo() -> Result<()> {
    let (payer, _) = demo_keypair("payer")?;
    let recipient = demo_keypair("recipient")?.0.pubkey();
    demo_genesis(&payer)?;

    println!(
        "The demo keys and genesis are in demo/, which git ignores.\n\n\
         Start the rollup:\n  \
         ROLLUP_GENESIS=demo/genesis.json cargo run -p rollup_core --locked\n\n\
         In a second terminal, pay the recipient and read its balance:\n  \
         cargo run -p rollup_client --locked -- demo/payer.json {recipient} {TRANSFER_LAMPORTS}\n  \
         curl http://127.0.0.1:8080/get_account/{recipient}"
    );
    Ok(())
}

/// The keypair in `demo/<name>.json`. It is created the first time it is asked for
/// and reused by every later run. Git ignores the folder, so the keys stay local.
fn demo_keypair(name: &str) -> Result<(Keypair, PathBuf)> {
    let directory = workspace_root().join("demo");
    fs::create_dir_all(&directory)?;
    let file = directory.join(format!("{name}.json"));

    let keypair = if file.exists() {
        read_keypair_file(&file)
            .map_err(|error| anyhow::anyhow!("reading {}: {error}", file.display()))?
    } else {
        let keypair = Keypair::new();
        write_keypair_file(&keypair, &file)
            .map_err(|error| anyhow::anyhow!("writing {}: {error}", file.display()))?;
        keypair
    };
    Ok((keypair, file))
}

/// Writes `demo/genesis.json`, which funds the demo payer.
fn demo_genesis(payer: &Keypair) -> Result<PathBuf> {
    let file = workspace_root().join("demo").join("genesis.json");
    let genesis = json!({"accounts": [
        {"address": payer.pubkey().to_string(), "lamports": GENESIS_LAMPORTS}
    ]});
    fs::write(&file, serde_json::to_vec_pretty(&genesis)?)?;
    Ok(file)
}

/// Runs one of the tests that plain `cargo test` skips because it needs the network.
fn ignored_test(name: &str) -> Result<()> {
    cargo(&[
        "test",
        "--package",
        "rollup_core",
        "--locked",
        name,
        "--",
        "--ignored",
        "--nocapture",
    ])
}

/// Sends two identical transfers with the real client to the real server, then checks
/// the receipts, the exact balances and a clean shutdown on SIGTERM.
fn smoke() -> Result<()> {
    cargo(&["build", "--workspace", "--locked"])?;

    let binaries = target_directory().join("debug");
    let artifacts = target_directory().join("verification").join("cli");
    fs::create_dir_all(&artifacts)?;

    // Phase 1, fixtures: the demo keys, created on the first run and reused after it.
    let (payer, payer_file) = demo_keypair("payer")?;
    let recipient = demo_keypair("recipient")?.0.pubkey();
    let genesis_file = demo_genesis(&payer)?;

    // Phase 2, server: start the rollup on a free loopback port and wait until it answers.
    let address = TcpListener::bind("127.0.0.1:0")?.local_addr()?;
    let url = format!("http://{address}");
    let log = fs::File::create(artifacts.join("server.log"))?;
    let mut server = Server(
        Command::new(binaries.join("rollup_core"))
            .env("ROLLUP_GENESIS", &genesis_file)
            .env("ROLLUP_BIND", address.to_string())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .spawn()
            .context("starting rollup_core")?,
    );
    let http = Client::builder().timeout(Duration::from_secs(5)).build()?;
    server.wait_until_ready(&http, &url)?;

    // Phase 3, transfers: the same payment twice must give two distinct transactions.
    let mut receipts = Vec::new();
    for _ in 0..TRANSFERS {
        let output = Command::new(binaries.join("rollup_client"))
            .arg(&payer_file)
            .arg(recipient.to_string())
            .arg(TRANSFER_LAMPORTS.to_string())
            .arg(&url)
            .output()
            .context("running rollup_client")?;
        ensure!(
            output.status.success(),
            "rollup_client failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let receipt: Value = serde_json::from_slice(&output.stdout)?;
        ensure!(receipt["status"] == "executed", "receipt: {receipt}");
        ensure!(receipt["version"] == 1, "client must send v1: {receipt}");
        ensure!(receipt["fee"] == FEE_LAMPORTS, "receipt: {receipt}");
        ensure!(receipt["settlement"] == "not_configured", "{receipt}");
        receipts.push(receipt);
    }
    ensure!(
        receipts[0]["signature"] != receipts[1]["signature"],
        "identical payments must still be distinct transactions"
    );

    // Phase 4, state: the balances must match the transfers and fees exactly.
    let balance = |address: String| -> Result<u64> {
        let account: Value = http
            .get(format!("{url}/get_account/{address}"))
            .send()?
            .error_for_status()?
            .json()?;
        account["lamports"].as_u64().context("lamport balance")
    };
    let payer_balance = balance(payer.pubkey().to_string())?;
    let recipient_balance = balance(recipient.to_string())?;
    let spent = TRANSFERS * (TRANSFER_LAMPORTS + FEE_LAMPORTS);
    ensure!(
        payer_balance == GENESIS_LAMPORTS - spent,
        "payer balance is {payer_balance}"
    );
    ensure!(
        recipient_balance == TRANSFERS * TRANSFER_LAMPORTS,
        "recipient balance is {recipient_balance}"
    );

    // Phase 5, shutdown: SIGTERM must end the server cleanly.
    let exit_code = server.terminate()?;
    ensure!(exit_code == 0, "SIGTERM shutdown exited with {exit_code}");

    let report = json!({
        "command": "cargo xtask smoke",
        "inputs": "The demo keypairs; demo/genesis.json; two identical v1 transfers from rollup_client",
        "payer": payer.pubkey().to_string(),
        "recipient": recipient.to_string(),
        "receipts": receipts,
        "final_balances": {"payer": payer_balance, "recipient": recipient_balance},
        "sigterm_exit_code": exit_code,
        "scope": "Real client, HTTP server and SVM on loopback; no validator, RPC network or L1 settlement",
        "result": "passed",
    });
    let report_file = artifacts.join("result.json");
    fs::write(&report_file, serde_json::to_vec_pretty(&report)?)?;

    println!(
        "Smoke test passed: two v1 transfers, exact balances, clean shutdown.\nReport: {}",
        report_file.display()
    );
    Ok(())
}

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives inside the workspace")
}

fn target_directory() -> PathBuf {
    env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root().join("target"))
}

fn cargo(arguments: &[&str]) -> Result<()> {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    println!("$ cargo {}", arguments.join(" "));

    let status = Command::new(cargo)
        .args(arguments)
        .current_dir(workspace_root())
        .status()
        .context("running cargo")?;
    ensure!(status.success(), "cargo {} failed", arguments.join(" "));
    Ok(())
}

/// The running rollup server; it is killed if a check fails before `terminate`.
struct Server(Child);

impl Server {
    fn wait_until_ready(&mut self, http: &Client, url: &str) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(status) = self.0.try_wait()? {
                bail!("rollup_core exited before it was ready: {status}; see server.log");
            }
            if http
                .get(url)
                .send()
                .is_ok_and(|response| response.status().is_success())
            {
                return Ok(());
            }
            ensure!(Instant::now() < deadline, "rollup_core readiness timed out");
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn terminate(&mut self) -> Result<i32> {
        let signalled = Command::new("kill")
            .arg("-TERM")
            .arg(self.0.id().to_string())
            .status()
            .context("sending SIGTERM")?;
        ensure!(signalled.success(), "could not signal rollup_core");

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.0.try_wait()? {
                return status.code().context("rollup_core was killed by a signal");
            }
            ensure!(Instant::now() < deadline, "rollup_core ignored SIGTERM");
            thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
