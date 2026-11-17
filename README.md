# Basic Rollup

A single-app SOL-transfer rollup demo. One Rust server contains the HTTP API,
sequencer, Agave SVM execution, account state and transaction receipts. A separate
client signs and submits transfers. No other service is required.

The application is native SOL transfers: it accepts legacy, v0 and v1 transactions
containing only System `Transfer` instructions. It is not a general
program-execution platform. Settlement on a Solana cluster is not built: it is the
[settlement challenge](#settlement-challenge).

## Requirements

- Rust 1.98.1 or newer. Nothing else: no Solana CLI and no validator.

## Run

Run every command from the project folder:

```sh
cd Basic_Rollup
```

Create the demo keys and a genesis file that funds the payer. They are saved in
`demo/`, which git ignores, and are reused by every later run and by the test tasks:

```sh
cargo xtask demo
```

It prints the commands below with the recipient's address filled in. Start the
server. It keeps running in this terminal:

```sh
ROLLUP_GENESIS=demo/genesis.json cargo run -p rollup_core --locked
```

In a second terminal, from the same folder, send 100000000 lamports (0.1 SOL) to the
address that `cargo xtask demo` printed:

```sh
cargo run -p rollup_client --locked -- demo/payer.json RECIPIENT_ADDRESS 100000000
```

The client prints the receipt. Read the recipient's balance, which is now
100000000 lamports:

```sh
curl http://127.0.0.1:8080/get_account/RECIPIENT_ADDRESS
```

- Amounts are integer lamports.
- The client's optional last argument is the rollup URL. It defaults to
  `http://127.0.0.1:8080`. `ROLLUP_BIND` changes the address the server listens on.
- The client signs a v1 transaction whose message config states the compute units
  and loaded account data a transfer needs.
- Accounts and receipts are in memory. Restarting the server resets them.

### The genesis file

Genesis is JSON containing `accounts`, each with a unique `address` and a `u64`
`lamports` balance. It creates System-owned accounts and cannot replace the System
program account. Its balances are synthetic lamports, not deposits. The SVM enforces
Mainnet's rent rules, so a newly funded recipient must receive at least 650240
lamports, the rent-exempt minimum of an empty account.

A Solana cluster starts the same way: its genesis config (`genesis.bin`, produced by
`solana-genesis`) holds the first funded accounts, the builtin programs and the
cluster parameters, and a validator builds its first bank from it.

### Pulling accounts from a cluster

Set `ROLLUP_CLUSTER_URL` to a Solana RPC URL (devnet, Mainnet or a local validator)
and the rollup copies an account from that cluster the first time a transaction or
a balance query touches it:

```sh
ROLLUP_CLUSTER_URL=https://api.devnet.solana.com cargo run -p rollup_core --locked
```

An account is pulled once. From then on the rollup reads and changes only its own
copy, so later changes on the cluster are not seen. With a cluster configured,
`ROLLUP_GENESIS` is optional, and a genesis account takes precedence over the
cluster's copy. Nothing is written back to the cluster and nothing locks the
account there: until settlement exists, the same lamports can still be spent on the
cluster.

## Run the tests

From the project folder, one command formats the code, runs Clippy with warnings
denied, runs every test and then the smoke test:

```sh
cd Basic_Rollup
```

```sh
cargo xtask verify
```

It needs no Solana CLI and no network. It ends with `Smoke test passed`. The smoke
test takes its keys from `demo/` and creates the ones that are missing.

The parts can be run on their own:

| Command | What it does | Needs |
| --- | --- | --- |
| `cargo test --workspace --locked` | The tests alone | Nothing else |
| `cargo xtask smoke` | Runs the real server and client: two identical transfers, exact balances, clean shutdown. Report in `target/verification/cli/` | Nothing else |
| `cargo xtask mainnet` | Compares the rent rate and the inactive features in `sequencer.rs` with Mainnet | Network |
| `cargo xtask live` | Starts the rollup on a real cluster and checks that a pulled account matches it. `ROLLUP_TEST_CLUSTER_URL` selects the cluster, default devnet | Network |

Run one test by its name:

```sh
cargo test -p rollup_core --locked failed_transactions_pay_the_fee_and_change_nothing_else
```

The tests in `rollup_core/tests/http_rollup.rs` start the real server on loopback
and execute signed transfers in the SVM. Each prints the path of its genesis and
server log.

| Test | What it checks |
| --- | --- |
| `legacy_transfers_commit_state_and_reject_replays` | Balances, fees, new recipients, repeated payments with fresh blockhashes, replays |
| `v1_transactions_carry_their_budget_and_reach_4096_bytes` | v1 budgets and priority fees, a 60-recipient payout, the size limit |
| `failed_transactions_pay_the_fee_and_change_nothing_else` | A failed transaction keeps only its fee payment |
| `invalid_submissions_are_rejected_without_a_fee` | Bad signatures, messages, blockhashes and non-transfer instructions |
| `accounts_are_pulled_from_the_cluster_once` | Pulling from a stand-in cluster |

## How it works

The rollup executes with **Agave SVM 4.4.1**, the validator release Solana Mainnet
runs under Solana 1.0. Sanitization, signature verification, budget and fee
derivation, account loading and execution run through the same crates a validator
uses (`solana-runtime-transaction`, `solana-fee`, `solana-svm`). The Agave crates
are pinned to one version in the workspace `Cargo.toml` and need the
`agave-unstable-api` feature.

Comments marked `Agave:` in `rollup_core/src` link each phase of the sequencer to
the code that does the same job in the validator, at tag v4.4.1: the banking-stage
scheduler, account locking, preparing the SVM, executing a transaction in it and
committing the result.

The diagrams are in [docs/architecture](docs/architecture/README.md).

### Mainnet parameters

The rollup executes one transaction at a time on a single thread, but every limit
it enforces is the Mainnet value, taken from the official crates wherever they
export it. The values live at the top of `rollup_core/src/sequencer.rs`.

| Parameter | Value | Source |
| --- | --- | --- |
| Transaction size, v1 | 4096 bytes | `solana_message::v1::MAX_TRANSACTION_SIZE` |
| Transaction size, legacy and v0 | 1232 bytes | `solana_packet::PACKET_DATA_SIZE` |
| v1 accounts, instructions, signatures | 64, 64, 12 | `solana_message::v1` sanitization |
| Account locks per transaction | 64 | Mainnet has not activated the raise to 128 |
| Fee per signature | 5000 lamports | `FeeStructure::default()`, via `solana_fee` |
| Priority fee | v1: total lamports in the message config | `solana_runtime_transaction` |
| Compute budget | v1: from the message config, unset means 0 | `solana_runtime_transaction` |
| Compute budget, legacy and v0 | 3000 units per builtin instruction | `solana_compute_budget_instruction` |
| Rent | 5080 lamports per byte, 650240 for an empty account | `set_lamports_per_byte_to_5080` |
| Blockhash validity | the 150 most recent hashes | `solana_clock::MAX_PROCESSING_AGE` |
| Feature set | every Agave 4.4.1 feature except 37 not yet active | snapshot of 2026-11-16 |

Two values are a snapshot of the live cluster rather than a crate constant: the
rent rate and the list of inactive features. `cargo xtask mainnet` compares both
with Mainnet and fails when either has changed.

### Differences from a validator

- No blocks or slots. A fresh blockhash is issued per request.
- No address lookup tables.
- No ComputeBudget program. Legacy and v0 transactions run with the default limits;
  v1 states its own.
- Fees are removed from the paying account without being burned or paid to a leader.
- No settlement. Receipts report `settlement: "not_configured"`.

## Settlement challenge

Settlement is left open as an exercise: design and build the on-chain program and
the rollup code that commit lamports on the cluster and let a wallet leave with its
rollup balance. The task, the cluster rules to build on and the decisions to make
are in [docs/settlement-challenge.md](docs/settlement-challenge.md).

## HTTP API

| Method and path | Input | Successful output |
| --- | --- | --- |
| `GET /` | None | Health, application, settlement status |
| `GET /get_latest_blockhash` | None | A fresh rollup `blockhash` |
| `GET /get_account/{address}` | Base58 public key | `address`, `lamports` (pulled from the cluster if unseen) |
| `POST /submit_transaction` | `{"sol_transaction": "<base64 serialized transaction>"}` | Execution receipt |
| `POST /get_transaction` | `{"get_tx": "<transaction signature>"}` | Stored execution receipt |

`sol_transaction` is the base64 of the serialized transaction, the same bytes
Solana's `sendTransaction` takes. Receipts are looked up by the first transaction
signature. Submission waits for execution and state commit.

A receipt contains `signature`, `version` (`"legacy"`, `0` or `1`), `status`
(`"executed"` or `"failed"`), `error` when it failed, `fee`,
`compute_units_consumed`, `settlement` and `sol_transaction`.

| HTTP status | Meaning |
| --- | --- |
| 200 | The transaction landed, executed or failed. As on Mainnet, a failed transaction pays its fee, keeps a receipt and cannot be replayed; every other change is undone |
| 400 | Malformed, oversized, unsigned or not a transfer |
| 404 | Unknown account or receipt |
| 409 | Replayed transaction |
| 422 | The fee payer cannot cover the fee |
| 503 | The cluster did not answer |

Only a 200 charges a fee: 5000 lamports per signature plus any v1 priority fee. An
account left with no lamports is removed.

The server accepts the latest 150 blockhashes it issued, so a client fetches one
right before signing. Every request issues a fresh hash, which gives identical
payments distinct signatures.
