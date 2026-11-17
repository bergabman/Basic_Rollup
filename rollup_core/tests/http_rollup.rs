//! End-to-end tests: the real server binary, real HTTP on loopback, real SVM execution.

use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::PathBuf,
    process::{Child, Command, Stdio},
    str::FromStr,
    sync::{Barrier, Mutex},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};
use base64::{prelude::BASE64_STANDARD, Engine};
use reqwest::{blocking::Client, StatusCode};
use serde_json::{json, Value};
use solana_hash::Hash;
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_message::{
    v1::{self, TransactionConfig},
    VersionedMessage,
};
use solana_packet::PACKET_DATA_SIZE;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_system_interface::{instruction::SystemInstruction, program};
use solana_transaction::{versioned::VersionedTransaction, Transaction};

const INITIAL_PAYER_BALANCE: u64 = 10_000_000_000;
const INITIAL_RECIPIENT_BALANCE: u64 = 1_000_000_000;

/// Mainnet charges 5000 lamports per signature.
const FEE: u64 = 5_000;

/// Mainnet's minimum balance for an account without data: 128 bytes at 5080 lamports.
const RENT_EXEMPT_MINIMUM: u64 = 650_240;

/// What the System program charges for one transfer.
const TRANSFER_COMPUTE_UNITS: u32 = 150;

/// One 32 KiB page, enough for every account a batch of transfers loads.
const LOADED_ACCOUNTS_DATA_SIZE: u32 = 32 * 1024;

/// Picking a free port and handing it to the server is not atomic, so parallel tests
/// start their servers one at a time.
static SERVER_STARTUP: Mutex<()> = Mutex::new(());

/// A running rollup server with a funded payer and one existing recipient.
struct Rollup {
    child: Child,
    url: String,
    client: Client,
    payer: Keypair,
    recipient: Keypair,
}

impl Rollup {
    /// Starts a rollup whose two accounts come from a genesis file.
    fn start(test_name: &str) -> Result<Self> {
        Self::spawn(test_name, None)
    }

    /// Starts a rollup without genesis: every account is pulled from `cluster_url`.
    fn start_on_cluster(test_name: &str, cluster_url: &str) -> Result<Self> {
        Self::spawn(test_name, Some(cluster_url))
    }

    fn spawn(test_name: &str, cluster_url: Option<&str>) -> Result<Self> {
        // These public, deterministic seeds are exclusively for isolated test balances.
        let payer = Keypair::new_from_array([7; 32]);
        let recipient = Keypair::new_from_array([8; 32]);

        let run_id = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let artifacts: PathBuf =
            std::env::temp_dir().join(format!("basic-rollup-e2e-{test_name}-{run_id}"));
        fs::create_dir(&artifacts)?;
        let mut server = Command::new(env!("CARGO_BIN_EXE_rollup_core"));
        match cluster_url {
            Some(cluster_url) => server.env("ROLLUP_CLUSTER_URL", cluster_url),
            None => {
                let genesis = json!({"accounts": [
                    {"address": payer.pubkey().to_string(), "lamports": INITIAL_PAYER_BALANCE},
                    {"address": recipient.pubkey().to_string(), "lamports": INITIAL_RECIPIENT_BALANCE}
                ]});
                let genesis_path = artifacts.join("genesis.json");
                fs::write(&genesis_path, serde_json::to_vec_pretty(&genesis)?)?;
                server.env("ROLLUP_GENESIS", genesis_path)
            }
        };
        let log_path = artifacts.join("server.log");
        let log = File::create(&log_path)?;
        println!("{test_name} artifacts: {}", artifacts.display());

        let _startup = SERVER_STARTUP
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        // Release the reserved port immediately before spawning the real server.
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        drop(listener);
        let child = server
            .env("ROLLUP_BIND", address.to_string())
            .env("RUST_LOG", "info")
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .spawn()
            .context("starting the rollup HTTP server")?;

        let mut rollup = Self {
            child,
            url: format!("http://{address}"),
            client: Client::builder()
                .connect_timeout(Duration::from_millis(250))
                .timeout(Duration::from_secs(5))
                .build()?,
            payer,
            recipient,
        };

        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(status) = rollup.child.try_wait()? {
                bail!(
                    "rollup exited before readiness: {status}; log:\n{}",
                    fs::read_to_string(&log_path)?
                );
            }
            if rollup
                .client
                .get(&rollup.url)
                .send()
                .is_ok_and(|response| response.status() == StatusCode::OK)
            {
                return Ok(rollup);
            }
            if Instant::now() >= deadline {
                bail!("rollup readiness timed out; log: {}", log_path.display());
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn post(&self, path: &str, body: &Value) -> Result<(StatusCode, Value)> {
        let response = self
            .client
            .post(format!("{}{path}", self.url))
            .json(body)
            .send()?;
        let status = response.status();
        let text = response.text()?;
        let body = serde_json::from_str(&text)
            .with_context(|| format!("expected JSON from {path}, status {status}: {text}"))?;

        Ok((status, body))
    }

    fn blockhash(&self) -> Result<Hash> {
        let body: Value = self
            .client
            .get(format!("{}/get_latest_blockhash", self.url))
            .send()?
            .error_for_status()?
            .json()?;

        Ok(Hash::from_str(
            body["blockhash"].as_str().context("blockhash response")?,
        )?)
    }

    fn submit(&self, wire_transaction: &[u8]) -> Result<(StatusCode, Value)> {
        self.post(
            "/submit_transaction",
            &json!({
                "sender": "E2E fixture",
                "sol_transaction": BASE64_STANDARD.encode(wire_transaction),
            }),
        )
    }

    fn receipt(&self, signature: &str) -> Result<(StatusCode, Value)> {
        self.post("/get_transaction", &json!({"get_tx": signature}))
    }

    /// The account's lamports, or `None` when the rollup has no such account.
    fn balance(&self, address: &Pubkey) -> Result<Option<u64>> {
        let response = self
            .client
            .get(format!("{}/get_account/{address}", self.url))
            .send()?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }

        let account: Value = response.error_for_status()?.json()?;
        assert_eq!(account["address"], address.to_string());
        Ok(account["lamports"].as_u64())
    }

    fn payer_balance(&self) -> Result<Option<u64>> {
        self.balance(&self.payer.pubkey())
    }

    fn transfer(&self, recipient: &Pubkey, lamports: u64) -> Instruction {
        Instruction::new_with_bincode(
            program::ID,
            &SystemInstruction::Transfer { lamports },
            vec![
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new(*recipient, false),
            ],
        )
    }

    fn legacy(&self, instructions: &[Instruction], blockhash: Hash) -> Transaction {
        Transaction::new_signed_with_payer(
            instructions,
            Some(&self.payer.pubkey()),
            &[&self.payer],
            blockhash,
        )
    }

    fn v1(
        &self,
        instructions: &[Instruction],
        blockhash: Hash,
        config: TransactionConfig,
    ) -> Result<VersionedTransaction> {
        let message = v1::Message::try_compile_with_config(
            &self.payer.pubkey(),
            instructions,
            blockhash,
            config,
        )?;

        Ok(VersionedTransaction::try_new(
            VersionedMessage::V1(message),
            &[&self.payer],
        )?)
    }
}

impl Drop for Rollup {
    fn drop(&mut self) {
        if let Err(error) = self.child.kill() {
            eprintln!("could not stop E2E server: {error}");
        }
        if let Err(error) = self.child.wait() {
            eprintln!("could not reap E2E server: {error}");
        }
    }
}

/// A stand-in Solana cluster: it answers every `getMultipleAccounts` request with
/// `lamports` in a System-owned account for `funded`, and "no account" for the rest.
fn stand_in_cluster(funded: Pubkey, lamports: u64) -> Result<String> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let url = format!("http://{}", listener.local_addr()?);

    thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
            let mut content_length = 0;
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|read| read > 2) {
                if let Some(length) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    content_length = length.trim().parse().expect("content length");
                }
                line.clear();
            }
            let mut body = vec![0; content_length];
            reader.read_exact(&mut body).expect("request body");
            let request: Value = serde_json::from_slice(&body).expect("JSON-RPC request");

            let accounts: Vec<Value> = request["params"][0]
                .as_array()
                .expect("addresses")
                .iter()
                .map(|address| {
                    if address == &json!(funded.to_string()) {
                        json!({"lamports": lamports, "data": ["", "base64"], "space": 0,
                            "owner": program::ID.to_string(), "executable": false, "rentEpoch": 0})
                    } else {
                        Value::Null
                    }
                })
                .collect();
            let response = json!({"jsonrpc": "2.0", "id": request["id"],
                "result": {"context": {"slot": 1}, "value": accounts}})
            .to_string();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
                 connection: close\r\n\r\n{response}",
                response.len()
            );
        }
    });

    Ok(url)
}

fn wire(transaction: &VersionedTransaction) -> Result<Vec<u8>> {
    Ok(wincode::serialize(transaction)?)
}

fn legacy_wire(transaction: &Transaction) -> Result<Vec<u8>> {
    wire(&VersionedTransaction::from(transaction.clone()))
}

/// The limits a v1 transaction needs for this many transfers.
fn transfer_config(transfers: u32) -> TransactionConfig {
    TransactionConfig::empty()
        .with_compute_unit_limit(transfers * TRANSFER_COMPUTE_UNITS)
        .with_loaded_accounts_data_size_limit(LOADED_ACCOUNTS_DATA_SIZE)
}

#[test]
fn legacy_transfers_commit_state_and_reject_replays() -> Result<()> {
    let rollup = Rollup::start("legacy")?;
    let recipient = rollup.recipient.pubkey();
    let new_recipient = Keypair::new_from_array([9; 32]).pubkey();
    let blockhash = rollup.blockhash()?;

    // A signed transfer executes, pays the fee and returns a receipt.
    let first_amount = 100_000_000;
    let first = rollup.legacy(&[rollup.transfer(&recipient, first_amount)], blockhash);
    let (status, first_receipt) = rollup.submit(&legacy_wire(&first)?)?;
    assert_eq!(status, StatusCode::OK, "first transfer: {first_receipt}");
    assert_eq!(first_receipt["signature"], first.signatures[0].to_string());
    assert_eq!(first_receipt["version"], "legacy");
    assert_eq!(first_receipt["status"], "executed");
    assert_eq!(first_receipt["fee"], FEE);
    assert_eq!(
        first_receipt["compute_units_consumed"],
        TRANSFER_COMPUTE_UNITS
    );
    assert_eq!(first_receipt["settlement"], "not_configured");
    assert_eq!(
        rollup.payer_balance()?,
        Some(INITIAL_PAYER_BALANCE - first_amount - FEE),
        "execution must commit the payer's transfer and fee"
    );
    assert_eq!(
        rollup.balance(&recipient)?,
        Some(INITIAL_RECIPIENT_BALANCE + first_amount)
    );

    // The same transaction cannot run twice.
    let (status, replay) = rollup.submit(&legacy_wire(&first)?)?;
    assert_eq!(status, StatusCode::CONFLICT, "replay: {replay}");
    assert_eq!(
        rollup.payer_balance()?,
        Some(INITIAL_PAYER_BALANCE - first_amount - FEE),
        "a replay must not debit a second time"
    );

    // A later transaction sees the state the earlier one committed.
    let second_amount = 200_000_000;
    let second = rollup.legacy(&[rollup.transfer(&new_recipient, second_amount)], blockhash);
    let (status, second_receipt) = rollup.submit(&legacy_wire(&second)?)?;
    assert_eq!(status, StatusCode::OK, "new recipient: {second_receipt}");
    let mut payer_balance = INITIAL_PAYER_BALANCE - first_amount - second_amount - 2 * FEE;
    assert_eq!(rollup.payer_balance()?, Some(payer_balance));
    assert_eq!(rollup.balance(&new_recipient)?, Some(second_amount));

    // Concurrent lookups each get their own receipt back.
    thread::scope(|scope| -> Result<()> {
        let mut lookups = Vec::new();
        for transaction in [&first, &second, &first, &second] {
            let rollup = &rollup;
            lookups.push(scope.spawn(move || -> Result<()> {
                let signature = transaction.signatures[0].to_string();
                let (status, receipt) = rollup.receipt(&signature)?;
                assert_eq!(status, StatusCode::OK, "lookup: {receipt}");
                assert_eq!(receipt["signature"], signature, "response correlation");
                assert_eq!(receipt["status"], "executed");
                assert_eq!(
                    receipt["sol_transaction"],
                    BASE64_STANDARD.encode(legacy_wire(transaction)?)
                );
                Ok(())
            }));
        }
        for lookup in lookups {
            lookup.join().expect("lookup thread panicked")?;
        }
        Ok(())
    })?;

    // A fresh blockhash makes an identical payment a distinct transaction.
    let fresh_hash = rollup.blockhash()?;
    assert_ne!(fresh_hash, blockhash);
    let repeated = rollup.legacy(&[rollup.transfer(&recipient, first_amount)], fresh_hash);
    assert_ne!(repeated.signatures[0], first.signatures[0]);
    let (status, repeated_receipt) = rollup.submit(&legacy_wire(&repeated)?)?;
    assert_eq!(status, StatusCode::OK, "repeat: {repeated_receipt}");
    payer_balance -= first_amount + FEE;
    assert_eq!(rollup.payer_balance()?, Some(payer_balance));
    assert_eq!(
        rollup.balance(&recipient)?,
        Some(INITIAL_RECIPIENT_BALANCE + 2 * first_amount)
    );

    // Two concurrent submissions of one transaction execute exactly once.
    let concurrent_amount = 3_000_000;
    let concurrent =
        legacy_wire(&rollup.legacy(&[rollup.transfer(&recipient, concurrent_amount)], blockhash))?;
    let barrier = Barrier::new(2);
    let results = thread::scope(|scope| -> Result<Vec<(StatusCode, Value)>> {
        let mut submissions = Vec::new();
        for _ in 0..2 {
            let (rollup, concurrent, barrier) = (&rollup, &concurrent, &barrier);
            submissions.push(scope.spawn(move || {
                barrier.wait();
                rollup.submit(concurrent)
            }));
        }
        submissions
            .into_iter()
            .map(|submission| submission.join().expect("submission thread panicked"))
            .collect()
    })?;
    let count = |expected: StatusCode| {
        results
            .iter()
            .filter(|(status, _)| *status == expected)
            .count()
    };
    assert_eq!(count(StatusCode::OK), 1, "one must execute: {results:?}");
    assert_eq!(count(StatusCode::CONFLICT), 1, "one must be a replay");
    payer_balance -= concurrent_amount + FEE;
    assert_eq!(rollup.payer_balance()?, Some(payer_balance));
    assert_eq!(
        rollup.balance(&recipient)?,
        Some(INITIAL_RECIPIENT_BALANCE + 2 * first_amount + concurrent_amount)
    );

    Ok(())
}

#[test]
fn v1_transactions_carry_their_budget_and_reach_4096_bytes() -> Result<()> {
    let rollup = Rollup::start("v1")?;
    let recipient = rollup.recipient.pubkey();
    let blockhash = rollup.blockhash()?;

    // A v1 transfer runs within the limits its own message config states.
    let amount = 100_000_000;
    let transfer = rollup.v1(
        &[rollup.transfer(&recipient, amount)],
        blockhash,
        transfer_config(1),
    )?;
    let (status, receipt) = rollup.submit(&wire(&transfer)?)?;
    assert_eq!(status, StatusCode::OK, "v1 transfer: {receipt}");
    assert_eq!(receipt["version"], 1);
    assert_eq!(receipt["status"], "executed");
    assert_eq!(receipt["fee"], FEE);
    assert_eq!(receipt["compute_units_consumed"], TRANSFER_COMPUTE_UNITS);
    let mut payer_balance = INITIAL_PAYER_BALANCE - amount - FEE;
    assert_eq!(rollup.payer_balance()?, Some(payer_balance));

    // The v1 priority fee is a total in lamports, added to the signature fee.
    let priority_fee = 1_234;
    let prioritized = rollup.v1(
        &[rollup.transfer(&recipient, amount)],
        blockhash,
        transfer_config(1).with_priority_fee(priority_fee),
    )?;
    let (status, receipt) = rollup.submit(&wire(&prioritized)?)?;
    assert_eq!(status, StatusCode::OK, "priority fee: {receipt}");
    assert_eq!(receipt["fee"], FEE + priority_fee);
    payer_balance -= amount + FEE + priority_fee;
    assert_eq!(rollup.payer_balance()?, Some(payer_balance));

    // A payout to 60 new accounts does not fit a legacy transaction...
    let payout = 1_000_000;
    let recipients: Vec<Pubkey> = (0..60).map(|_| Keypair::new().pubkey()).collect();
    let batch: Vec<Instruction> = recipients
        .iter()
        .map(|recipient| rollup.transfer(recipient, payout))
        .collect();
    let legacy_batch = legacy_wire(&rollup.legacy(&batch, blockhash))?;
    assert!(legacy_batch.len() > PACKET_DATA_SIZE);
    let (status, response) = rollup.submit(&legacy_batch)?;
    assert_eq!(status, StatusCode::BAD_REQUEST, "legacy batch: {response}");

    // ...but it is one atomic v1 transaction.
    let v1_batch = wire(&rollup.v1(&batch, blockhash, transfer_config(60))?)?;
    assert!(v1_batch.len() > PACKET_DATA_SIZE && v1_batch.len() <= v1::MAX_TRANSACTION_SIZE);
    let (status, receipt) = rollup.submit(&v1_batch)?;
    assert_eq!(status, StatusCode::OK, "v1 batch: {receipt}");
    assert_eq!(receipt["status"], "executed");
    assert_eq!(
        receipt["compute_units_consumed"],
        60 * TRANSFER_COMPUTE_UNITS
    );
    payer_balance -= 60 * payout + FEE;
    assert_eq!(rollup.payer_balance()?, Some(payer_balance));
    for recipient in &recipients {
        assert_eq!(rollup.balance(recipient)?, Some(payout));
    }

    // Anything above 4096 bytes is refused before it is even decoded.
    let mut oversized = vec![v1::V1_PREFIX];
    oversized.resize(v1::MAX_TRANSACTION_SIZE + 1, 0);
    let (status, response) = rollup.submit(&oversized)?;
    assert_eq!(status, StatusCode::BAD_REQUEST, "oversized v1: {response}");

    // Unset v1 limits are zero, so the transaction lands, fails and still pays its fee.
    let unbudgeted = rollup.v1(
        &[rollup.transfer(&recipient, amount)],
        blockhash,
        TransactionConfig::empty(),
    )?;
    let (status, receipt) = rollup.submit(&wire(&unbudgeted)?)?;
    assert_eq!(status, StatusCode::OK, "unbudgeted v1: {receipt}");
    assert_eq!(receipt["status"], "failed");
    assert_eq!(receipt["fee"], FEE);
    payer_balance -= FEE;
    assert_eq!(rollup.payer_balance()?, Some(payer_balance));

    Ok(())
}

#[test]
fn failed_transactions_pay_the_fee_and_change_nothing_else() -> Result<()> {
    let rollup = Rollup::start("failed")?;
    let recipient = rollup.recipient.pubkey();
    let new_recipient = Keypair::new_from_array([9; 32]).pubkey();
    let blockhash = rollup.blockhash()?;

    let overspend = rollup.legacy(&[rollup.transfer(&recipient, u64::MAX)], blockhash);
    let second_instruction_fails = rollup.legacy(
        &[
            rollup.transfer(&recipient, 10_000_000),
            rollup.transfer(&new_recipient, u64::MAX),
        ],
        blockhash,
    );
    let source_did_not_sign = rollup.legacy(
        &[Instruction::new_with_bincode(
            program::ID,
            &SystemInstruction::Transfer {
                lamports: 5_000_000,
            },
            vec![
                AccountMeta::new(recipient, false),
                AccountMeta::new(new_recipient, false),
            ],
        )],
        blockhash,
    );
    // Mainnet needs 650240 lamports in a new account; one lamport less is refused.
    let below_rent_exemption = rollup.legacy(
        &[rollup.transfer(&new_recipient, RENT_EXEMPT_MINIMUM - 1)],
        blockhash,
    );

    let mut payer_balance = INITIAL_PAYER_BALANCE;
    for (case, transaction) in [
        ("overspend", &overspend),
        ("second instruction fails", &second_instruction_fails),
        ("transfer source did not sign", &source_did_not_sign),
        ("below rent exemption", &below_rent_exemption),
    ] {
        let signature = transaction.signatures[0].to_string();
        let (status, receipt) = rollup.submit(&legacy_wire(transaction)?)?;
        assert_eq!(status, StatusCode::OK, "{case}: {receipt}");
        assert_eq!(receipt["status"], "failed", "{case}: {receipt}");
        assert_eq!(receipt["fee"], FEE, "{case}");
        assert!(receipt["error"].is_string(), "{case}: {receipt}");

        payer_balance -= FEE;
        assert_eq!(
            rollup.payer_balance()?,
            Some(payer_balance),
            "{case}: only the fee may leave the payer"
        );
        assert_eq!(
            rollup.balance(&recipient)?,
            Some(INITIAL_RECIPIENT_BALANCE),
            "{case}: every other change must be rolled back"
        );
        assert_eq!(rollup.balance(&new_recipient)?, None, "{case}");

        let (status, stored) = rollup.receipt(&signature)?;
        assert_eq!(status, StatusCode::OK, "{case}: failed receipt is stored");
        assert_eq!(stored["status"], "failed");
        let (status, replay) = rollup.submit(&legacy_wire(transaction)?)?;
        assert_eq!(status, StatusCode::CONFLICT, "{case} replay: {replay}");
    }

    // Exactly the rent-exempt minimum creates the account.
    let rent_exempt = rollup.legacy(
        &[rollup.transfer(&new_recipient, RENT_EXEMPT_MINIMUM)],
        blockhash,
    );
    let (status, receipt) = rollup.submit(&legacy_wire(&rent_exempt)?)?;
    assert_eq!(status, StatusCode::OK, "rent exempt: {receipt}");
    assert_eq!(receipt["status"], "executed");
    assert_eq!(rollup.balance(&new_recipient)?, Some(RENT_EXEMPT_MINIMUM));

    // A payer that cannot cover the fee never lands: no charge and no receipt.
    let unfunded = Keypair::new();
    let unfunded_transfer = Transaction::new_signed_with_payer(
        &[Instruction::new_with_bincode(
            program::ID,
            &SystemInstruction::Transfer { lamports: 1 },
            vec![
                AccountMeta::new(unfunded.pubkey(), true),
                AccountMeta::new(recipient, false),
            ],
        )],
        Some(&unfunded.pubkey()),
        &[&unfunded],
        blockhash,
    );
    let (status, response) = rollup.submit(&legacy_wire(&unfunded_transfer)?)?;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "unfunded payer: {response}"
    );
    let (status, _) = rollup.receipt(&unfunded_transfer.signatures[0].to_string())?;
    assert_eq!(status, StatusCode::NOT_FOUND);

    Ok(())
}

#[test]
fn invalid_submissions_are_rejected_without_a_fee() -> Result<()> {
    let rollup = Rollup::start("invalid")?;
    let recipient = rollup.recipient.pubkey();
    let blockhash = rollup.blockhash()?;

    let mut tampered = rollup.legacy(&[rollup.transfer(&recipient, 1_000_000)], blockhash);
    tampered.message.instructions[0].data[4] ^= 1;
    let mut malformed = rollup.legacy(&[rollup.transfer(&recipient, 2_000_000)], blockhash);
    malformed.message.instructions[0].accounts[0] = u8::MAX;
    malformed.sign(&[&rollup.payer], blockhash);
    let wrong_hash = rollup.legacy(
        &[rollup.transfer(&recipient, 3_000_000)],
        Hash::new_from_array([42; 32]),
    );
    let mut unsupported_program = rollup.transfer(&recipient, 4_000_000);
    unsupported_program.program_id = Keypair::new().pubkey();
    let non_transfer = Instruction::new_with_bincode(
        program::ID,
        &SystemInstruction::Assign { owner: recipient },
        vec![AccountMeta::new(rollup.payer.pubkey(), true)],
    );

    for (case, transaction) in [
        ("tampered signature", tampered),
        ("out-of-bounds account index", malformed),
        ("wrong blockhash", wrong_hash),
        (
            "unsupported program",
            rollup.legacy(&[unsupported_program], blockhash),
        ),
        (
            "non-transfer system instruction",
            rollup.legacy(&[non_transfer], blockhash),
        ),
        ("empty transaction", rollup.legacy(&[], blockhash)),
    ] {
        let (status, response) = rollup.submit(&legacy_wire(&transaction)?)?;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{case}: {response}");
    }

    let (status, response) = rollup.post(
        "/submit_transaction",
        &json!({"sol_transaction": "not base64!"}),
    )?;
    assert_eq!(status, StatusCode::BAD_REQUEST, "bad base64: {response}");
    let (status, response) = rollup.submit(b"not a transaction")?;
    assert_eq!(status, StatusCode::BAD_REQUEST, "bad bytes: {response}");

    assert_eq!(
        rollup.payer_balance()?,
        Some(INITIAL_PAYER_BALANCE),
        "rejected submissions must not charge a fee"
    );
    assert_eq!(rollup.balance(&recipient)?, Some(INITIAL_RECIPIENT_BALANCE));

    // Receipt lookups distinguish an unknown signature from a malformed one.
    let never_submitted = rollup.payer.sign_message(b"never submitted").to_string();
    let (status, response) = rollup.receipt(&never_submitted)?;
    assert_eq!(status, StatusCode::NOT_FOUND, "missing lookup: {response}");
    let (status, response) = rollup.receipt("not-a-signature")?;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "malformed lookup: {response}"
    );

    Ok(())
}

#[test]
fn accounts_are_pulled_from_the_cluster_once() -> Result<()> {
    let payer = Keypair::new_from_array([7; 32]).pubkey();
    let cluster_url = stand_in_cluster(payer, INITIAL_PAYER_BALANCE)?;
    let rollup = Rollup::start_on_cluster("cluster", &cluster_url)?;
    let new_recipient = Keypair::new().pubkey();

    // An account the rollup has never seen shows the cluster's balance.
    assert_eq!(rollup.payer_balance()?, Some(INITIAL_PAYER_BALANCE));
    assert_eq!(rollup.balance(&new_recipient)?, None);

    // A pulled account pays for a transfer like any other.
    let amount = 100_000_000;
    let transfer = rollup.v1(
        &[rollup.transfer(&new_recipient, amount)],
        rollup.blockhash()?,
        transfer_config(1),
    )?;
    let (status, receipt) = rollup.submit(&wire(&transfer)?)?;
    assert_eq!(status, StatusCode::OK, "transfer: {receipt}");
    assert_eq!(receipt["status"], "executed");

    // The cluster still reports the old balance; the rollup must keep its own copy.
    assert_eq!(
        rollup.payer_balance()?,
        Some(INITIAL_PAYER_BALANCE - amount - FEE),
        "a pulled account must not be pulled again"
    );
    assert_eq!(rollup.balance(&new_recipient)?, Some(amount));

    Ok(())
}

/// Run with `cargo xtask live`. `ROLLUP_TEST_CLUSTER_URL` selects the cluster and
/// defaults to devnet; plain `cargo test` skips it because it needs the network.
#[test]
#[ignore = "queries a live Solana cluster"]
fn live_cluster_account_matches_the_rollup() -> Result<()> {
    let cluster_url = std::env::var("ROLLUP_TEST_CLUSTER_URL")
        .unwrap_or_else(|_| "https://api.devnet.solana.com".into());
    let rollup = Rollup::start_on_cluster("live", &cluster_url)?;

    // The rent sysvar exists on every cluster and nobody can spend from it.
    let address = Pubkey::from_str("SysvarRent111111111111111111111111111111111")?;
    let cluster: Value = rollup
        .client
        .post(&cluster_url)
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "getBalance", "params": [address.to_string()]}))
        .send()?
        .error_for_status()?
        .json()?;

    let lamports = cluster["result"]["value"]
        .as_u64()
        .context("cluster balance")?;
    assert_eq!(rollup.balance(&address)?, Some(lamports));
    println!(
        "{cluster_url}: {address} holds {lamports} lamports, on the cluster and in the rollup"
    );

    Ok(())
}
