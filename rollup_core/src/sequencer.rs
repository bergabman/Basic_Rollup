use std::{
    collections::{HashSet, VecDeque},
    slice,
    sync::{Arc, RwLock},
};

use agave_feature_set::{self as feature, FeatureSet};
use agave_reserved_account_keys::ReservedAccountKeys;
use anyhow::{anyhow, Result};
use base64::{prelude::BASE64_STANDARD, Engine};
use solana_account::AccountSharedData;
use solana_bincode::limited_deserialize;
use solana_clock::{Slot, MAX_PROCESSING_AGE};
use solana_fee::FeeFeatures;
use solana_fee_structure::FeeStructure;
use solana_hash::Hash;
use solana_keypair::Keypair;
use solana_message::{v1, SanitizedMessage, SimpleAddressLoader};
use solana_packet::PACKET_DATA_SIZE;
use solana_program_runtime::{
    execution_budget::{SVMTransactionExecutionAndFeeBudgetLimits, SVMTransactionExecutionBudget},
    loaded_programs::{BlockRelation, ForkGraph, ProgramRuntimeEnvironments},
    program_cache_entry::ProgramCacheEntry,
    solana_sbpf::program::BuiltinFunctionDefinition,
};
use solana_pubkey::Pubkey;
use solana_rent::Rent;
use solana_runtime_transaction::{
    runtime_transaction::RuntimeTransaction, transaction_meta::TransactionMeta,
};
use solana_sdk_ids::system_program;
use solana_signature::Signature;
use solana_signer::Signer;
use solana_svm::{
    account_loader::CheckedTransactionDetails,
    transaction_processing_result::ProcessedTransaction,
    transaction_processor::{
        TransactionBatchProcessor, TransactionProcessingConfig, TransactionProcessingEnvironment,
    },
};
use solana_svm_transaction::svm_message::SVMStaticMessage;
use solana_system_interface::instruction::SystemInstruction;
use solana_system_program::system_processor;
use solana_transaction::{
    sanitized::{MessageHash, SanitizedTransaction, MAX_TX_ACCOUNT_LOCKS},
    versioned::{TransactionVersion, VersionedTransaction},
};
use tokio::sync::{mpsc, oneshot};

use crate::{
    rollupdb::{AccountBalance, ExecutionStatus, RollupDB, TransactionReceipt},
    settle::SettlementStatus,
};

// Solana Mainnet parameters (Solana 1.0, Agave 4.4.1). Limits that a crate exports
// are imported above: `PACKET_DATA_SIZE` (1232 bytes, legacy and v0),
// `v1::MAX_TRANSACTION_SIZE` (4096 bytes) and `MAX_PROCESSING_AGE` (150 blockhashes).
// The values below are the ones Mainnet sets through feature activations.

/// Mainnet rent since SIMD-0437's second reduction; `Rent::default()` still holds 6960.
const LAMPORTS_PER_BYTE: u64 = feature::set_lamports_per_byte_to_5080::LAMPORTS_PER_BYTE;

/// Account locks per transaction until `increase_tx_account_lock_limit` activates and
/// raises the limit to `MAX_TX_ACCOUNT_LOCKS`. The validator keeps this in its bank.
const BASE_TX_ACCOUNT_LOCKS: usize = 64;

/// Agave 4.4.1 features that Mainnet had not activated at slot 470188639 (2026-11-16).
/// `cargo xtask mainnet` compares this snapshot and the rent above with the live cluster.
const MAINNET_INACTIVE_FEATURES: [Pubkey; 37] = [
    feature::full_inflation::devnet_and_testnet::ID,
    feature::stake_raise_minimum_delegation_to_1_sol::ID,
    feature::blake3_syscall_enabled::ID,
    feature::zk_token_sdk_enabled::ID,
    feature::libsecp256k1_fail_on_bad_count::ID,
    feature::increase_tx_account_lock_limit::ID,
    feature::enable_big_mod_exp_syscall::ID,
    feature::virtual_address_space_adjustments::ID,
    feature::account_data_direct_mapping::ID,
    feature::include_loaded_accounts_data_size_in_fee_calculation::ID,
    feature::remaining_compute_units_syscall_enabled::ID,
    feature::enable_loader_v4::ID,
    feature::enable_zk_transfer_with_fee::ID,
    feature::enable_zk_proof_from_account::ID,
    feature::chained_merkle_conflict_duplicate_proofs::ID,
    feature::verify_retransmitter_signature::ID,
    feature::vote_only_retransmitter_signed_fec_sets::ID,
    feature::deprecate_legacy_vote_ixs::ID,
    feature::disable_sbpf_v0_execution::ID,
    feature::reenable_sbpf_v0_execution::ID,
    feature::disable_sbpf_v0_v1_v2_deployment::ID,
    feature::enable_extend_program_checked::ID,
    feature::raise_cpi_nesting_limit_to_8::ID,
    feature::enforce_correct_proof_size::ID,
    feature::set_lamports_per_byte_to_2575::ID,
    feature::set_lamports_per_byte_to_1322::ID,
    feature::set_lamports_per_byte_to_696::ID,
    feature::block_revenue_sharing::ID,
    feature::vote_account_initialize_v2::ID,
    feature::direct_account_pointers_in_program_input::ID,
    feature::enable_sha512_syscall::ID,
    feature::set_lamports_per_byte_to_6960::ID,
    feature::alpenglow_fast_leader_handover::ID,
    feature::relax_fee_payer_constraint::ID,
    feature::double_disinflation_rate::ID,
    feature::remove_inactive_stakes::ID,
    feature::loader_v3_set_program_data_to_elf_length::ID,
];

/// A transaction that passed the validator's sanitization and signature checks.
type ValidTransaction = RuntimeTransaction<SanitizedTransaction>;

pub(crate) enum SequencerMessage {
    Blockhash {
        reply: oneshot::Sender<Hash>,
    },
    Submit {
        /// The serialized transaction, exactly as it would travel to a validator.
        wire_transaction: Vec<u8>,
        reply: oneshot::Sender<Result<TransactionReceipt, SubmissionError>>,
    },
    Transaction {
        signature: Signature,
        reply: oneshot::Sender<Option<TransactionReceipt>>,
    },
    Balance {
        address: Pubkey,
        /// `Err` carries the reason the cluster could not be asked for the account.
        reply: oneshot::Sender<Result<Option<AccountBalance>, String>>,
    },
}

/// Why a submission never reached a receipt. None of these charge a fee.
#[derive(Debug)]
pub(crate) enum SubmissionError {
    /// Malformed, unsigned, expired or outside the SOL-transfer application.
    Invalid(String),
    AlreadyProcessed,
    /// The SVM could not charge the fee payer, so the transaction cannot land.
    Rejected(String),
    /// The cluster that accounts are pulled from did not answer.
    ClusterUnavailable(String),
}

impl SubmissionError {
    fn invalid(reason: impl ToString) -> Self {
        Self::Invalid(reason.to_string())
    }
}

// Lines marked `Agave:` name the code that does the same job in the Agave validator.
// Every link points at tag v4.4.1, the release the rollup's Solana crates come from.

// The rollup has one chain and no forks. A validator answers from its tree of banks.
// Agave: `impl ForkGraph for BankForks`
// https://github.com/anza-xyz/agave/blob/v4.4.1/runtime/src/bank_forks.rs#L730
struct RollupForkGraph;

impl ForkGraph for RollupForkGraph {
    fn relationship(&self, a: Slot, b: Slot) -> BlockRelation {
        if a == b {
            BlockRelation::Equal
        } else {
            BlockRelation::Unrelated
        }
    }
}

// The sequencer does alone what three parts of a validator do together.
// Agave: `SchedulerController`, the scheduler thread of the banking stage
// https://github.com/anza-xyz/agave/blob/v4.4.1/core/src/banking_stage/transaction_scheduler/scheduler_controller.rs#L145
// Agave: `ConsumeWorker`, a thread that executes what the scheduler hands it
// https://github.com/anza-xyz/agave/blob/v4.4.1/core/src/banking_stage/consume_worker.rs#L37
// Agave: `Bank`, the state of one block and the owner of the SVM
// https://github.com/anza-xyz/agave/blob/v4.4.1/runtime/src/bank.rs#L867

/// A single owner serializes execution and commits,
/// so readers cannot observe partial state.
pub(crate) struct Sequencer {
    db: RollupDB,
    recent_blockhashes: VecDeque<Hash>,
    feature_set: FeatureSet,
    reserved_account_keys: HashSet<Pubkey>,
    processor: TransactionBatchProcessor<RollupForkGraph>,
    environment: TransactionProcessingEnvironment,
    // The SVM program cache holds only a Weak reference to this graph.
    _fork_graph: Arc<RwLock<RollupForkGraph>>,
}

impl Sequencer {
    // Prepares the SVM: the active features, the program cache, the builtin programs
    // and the environment every transaction executes in.
    //
    // Agave: `Bank::compute_active_feature_set`, reads the features from their accounts
    // https://github.com/anza-xyz/agave/blob/v4.4.1/runtime/src/bank.rs#L6604
    // Agave: `TransactionBatchProcessor::new_uninitialized`, the processor a bank owns
    // https://github.com/anza-xyz/agave/blob/v4.4.1/svm/src/transaction_processor.rs#L282
    // Agave: `Bank::set_fork_graph_in_program_cache`
    // https://github.com/anza-xyz/agave/blob/v4.4.1/runtime/src/bank.rs#L1676
    // Agave: `Bank::add_builtin`, called for the programs listed in `BUILTINS`
    // https://github.com/anza-xyz/agave/blob/v4.4.1/runtime/src/bank.rs#L6800
    // https://github.com/anza-xyz/agave/blob/v4.4.1/builtins/src/lib.rs#L28
    // Agave: `create_program_runtime_environment`, the syscalls and VM settings that
    // on-chain programs run with. The rollup keeps the processor's empty default.
    // https://github.com/anza-xyz/agave/blob/v4.4.1/syscalls/src/lib.rs#L314
    // Agave: `Bank::do_load_and_execute_transactions` builds the environment per batch
    // https://github.com/anza-xyz/agave/blob/v4.4.1/runtime/src/bank.rs#L4276
    pub fn new(db: RollupDB) -> Result<Self> {
        let mut feature_set = FeatureSet::all_enabled();
        for feature_id in &MAINNET_INACTIVE_FEATURES {
            feature_set.deactivate(feature_id);
        }

        let mut reserved_account_keys = ReservedAccountKeys::default();
        reserved_account_keys.update_active_set(&feature_set);

        let processor = TransactionBatchProcessor::new_uninitialized(0, 0);
        let fork_graph = Arc::new(RwLock::new(RollupForkGraph));
        processor
            .global_program_cache
            .write()
            .map_err(|_| anyhow!("program cache lock poisoned"))?
            .set_fork_graph(Arc::downgrade(&fork_graph));
        processor.add_builtin(
            system_program::id(),
            ProgramCacheEntry::new_builtin(system_processor::Entrypoint::register),
        );

        // Native System transfers need no SBF loader or syscall environment.
        let runtime = processor.program_runtime_environment.clone();
        let environment = TransactionProcessingEnvironment {
            blockhash: Hash::default(),
            blockhash_lamports_per_signature: FeeStructure::default().lamports_per_signature,
            // Only the Vote program reads this flag; it follows the feature here.
            alpenglow_migration_succeeded: feature_set.is_active(&feature::alpenglow::ID),
            epoch_total_stake: 0,
            feature_set: feature_set.runtime_features(),
            program_runtime_environments: ProgramRuntimeEnvironments::new(runtime.clone(), runtime),
            rent: Rent::with_lamports_per_byte(LAMPORTS_PER_BYTE),
        };

        Ok(Self {
            db,
            recent_blockhashes: VecDeque::new(),
            feature_set,
            reserved_account_keys: reserved_account_keys.active,
            processor,
            environment,
            _fork_graph: fork_graph,
        })
    }

    // The scheduler. One thread takes transactions in arrival order and executes one
    // at a time, so two transactions never use an account at the same moment and the
    // rollup needs no account locks.
    //
    // A validator executes on several worker threads. Its scheduler orders the waiting
    // transactions by priority and gives a worker only transactions that do not
    // conflict with what another worker is executing. The worker's bank then locks
    // the accounts of the batch until it is committed.
    //
    // Agave: `SchedulerController::run`, the loop of the scheduler thread
    // https://github.com/anza-xyz/agave/blob/v4.4.1/core/src/banking_stage/transaction_scheduler/scheduler_controller.rs#L216
    // Agave: `GreedyScheduler::schedule`, picks by priority and fills a batch per worker
    // https://github.com/anza-xyz/agave/blob/v4.4.1/core/src/banking_stage/transaction_scheduler/greedy_scheduler.rs#L83
    // Agave: `ThreadAwareAccountLocks::try_lock_accounts`, the scheduler's account locks
    // https://github.com/anza-xyz/agave/blob/v4.4.1/scheduling-utils/src/thread_aware_account_locks.rs#L89
    // Agave: `ConsumeWorker::run`, the loop of a worker thread
    // https://github.com/anza-xyz/agave/blob/v4.4.1/core/src/banking_stage/consume_worker.rs#L70
    // Agave: `Bank::prepare_sanitized_batch_with_results`, locks a batch's accounts
    // https://github.com/anza-xyz/agave/blob/v4.4.1/runtime/src/bank.rs#L3926
    // Agave: `AccountLocks::try_lock_transaction_batch`, the bank's account locks
    // https://github.com/anza-xyz/agave/blob/v4.4.1/accounts-db/src/account_locks.rs#L22
    pub fn run(mut self, mut receiver: mpsc::Receiver<SequencerMessage>) {
        while let Some(message) = receiver.blocking_recv() {
            match message {
                SequencerMessage::Blockhash { reply } => {
                    let blockhash = self.issue_blockhash();
                    let _ = reply.send(blockhash);
                }
                SequencerMessage::Submit {
                    wire_transaction,
                    reply,
                } => {
                    log::info!("transaction arrived: {} bytes", wire_transaction.len());
                    let result = self.execute(&wire_transaction);
                    match &result {
                        Ok(receipt) => log::info!(
                            "transaction {} {}: fee {} lamports, {} compute units",
                            receipt.signature,
                            match &receipt.error {
                                Some(error) => format!("failed ({error})"),
                                None => "executed".into(),
                            },
                            receipt.fee,
                            receipt.compute_units_consumed
                        ),
                        Err(error) => log::warn!("transaction refused: {error:?}"),
                    }
                    if reply.send(result).is_err() {
                        log::debug!(
                            "submit caller disconnected; any committed receipt remains available"
                        );
                    }
                }
                SequencerMessage::Transaction { signature, reply } => {
                    let _ = reply.send(self.db.transaction(&signature));
                }
                SequencerMessage::Balance { address, reply } => {
                    let balance = match self.db.pull([&address]) {
                        Ok(()) => Ok(self.db.balance(&address)),
                        Err(error) => Err(format!("{error:#}")),
                    };
                    let _ = reply.send(balance);
                }
            }
        }
    }

    // Agave: `BlockhashQueue::register_hash`, a bank adds the hash of each finished block
    // https://github.com/anza-xyz/agave/blob/v4.4.1/accounts-db/src/blockhash_queue.rs#L133
    fn issue_blockhash(&mut self) -> Hash {
        // Fresh random hashes distinguish identical payments and invalidate prior sessions.
        let blockhash = Hash::new_from_array(Keypair::new().pubkey().to_bytes());

        // A validator accepts the last `MAX_PROCESSING_AGE` block hashes; so does the rollup.
        if self.recent_blockhashes.len() == MAX_PROCESSING_AGE {
            self.recent_blockhashes.pop_front();
        }
        self.recent_blockhashes.push_back(blockhash);

        blockhash
    }

    // A validator spreads these stages over threads. Phases 1, 2 and 4 run in the
    // sigverify stage and in check workers, before the scheduler sees the transaction.
    // Phases 5 and 6 run in the worker that the scheduler gave the transaction to.
    //
    // Agave: `Consumer::process_and_record_aged_transactions`, a worker takes one batch
    // through account locking, execution, recording and commit
    // https://github.com/anza-xyz/agave/blob/v4.4.1/core/src/banking_stage/consumer.rs#L173
    // Agave: `Consumer::execute_and_commit_transactions_locked`
    // https://github.com/anza-xyz/agave/blob/v4.4.1/core/src/banking_stage/consumer.rs#L260

    /// Takes one transaction through the same stages a validator runs.
    fn execute(&mut self, wire_transaction: &[u8]) -> Result<TransactionReceipt, SubmissionError> {
        let transaction = self.decode(wire_transaction)?;
        let transaction = self.validate(transaction)?;
        self.load(transaction.message().account_keys().iter())?;
        let checked = self.budget(&transaction)?;
        let processed = self.process(&transaction, checked)?;

        Ok(self.commit(wire_transaction, &transaction, processed))
    }

    // Phase 1, decode: wire bytes become a transaction within its version's size limit.
    //
    // Agave: `precheck_transaction`, everything a check worker tests on one packet
    // before the scheduler buffers it
    // https://github.com/anza-xyz/agave/blob/v4.4.1/core/src/banking_stage/transaction_scheduler/receive_and_buffer.rs#L64
    // Agave: `translate_to_runtime_view`, parses the packet bytes
    // https://github.com/anza-xyz/agave/blob/v4.4.1/core/src/banking_stage/transaction_scheduler/receive_and_buffer.rs#L125
    fn decode(&self, wire_transaction: &[u8]) -> Result<VersionedTransaction, SubmissionError> {
        // A v1 transaction starts with its version byte; older formats start with signatures.
        let is_v1 = wire_transaction.first() == Some(&v1::V1_PREFIX);
        let size_limit = if is_v1 {
            v1::MAX_TRANSACTION_SIZE
        } else {
            PACKET_DATA_SIZE
        };
        if wire_transaction.len() > size_limit {
            return Err(SubmissionError::Invalid(format!(
                "transaction is {} bytes; the wire limit for its version is {size_limit}",
                wire_transaction.len()
            )));
        }
        if is_v1 && !self.feature_set.is_active(&feature::enable_tx_v1::ID) {
            return Err(SubmissionError::invalid("v1 transactions are not enabled"));
        }

        wincode::deserialize_exact(wire_transaction).map_err(SubmissionError::invalid)
    }

    // Phase 2, validate: signatures, sanitization, account locks, blockhash age,
    // the SOL-transfer application rule and replay protection.
    //
    // Agave: `run_transaction_task`, the sigverify stage verifies the signatures
    // https://github.com/anza-xyz/agave/blob/v4.4.1/core/src/sigverify.rs#L266
    // Agave: `RuntimeTransaction::<SanitizedTransaction>::try_create`, the same call
    // https://github.com/anza-xyz/agave/blob/v4.4.1/runtime-transaction/src/runtime_transaction/sdk_transactions.rs#L79
    // Agave: `validate_account_locks`, refuses too many accounts or one account twice
    // https://github.com/anza-xyz/agave/blob/v4.4.1/accounts-db/src/account_locks.rs#L143
    // Agave: `Bank::check_blockhash_age`
    // https://github.com/anza-xyz/agave/blob/v4.4.1/runtime/src/bank/check_transactions.rs#L193
    // Agave: `Bank::check_status_cache`, replay protection, checked again at execution
    // https://github.com/anza-xyz/agave/blob/v4.4.1/runtime/src/bank/check_transactions.rs#L244
    fn validate(
        &self,
        transaction: VersionedTransaction,
    ) -> Result<ValidTransaction, SubmissionError> {
        let message_hash = transaction
            .verify_and_hash_message()
            .map_err(SubmissionError::invalid)?;

        // The rollup has no address lookup table program, so v0 lookups cannot resolve.
        let transaction = RuntimeTransaction::<SanitizedTransaction>::try_create(
            transaction,
            MessageHash::Precomputed(message_hash),
            None,
            SimpleAddressLoader::Disabled,
            &self.reserved_account_keys,
        )
        .map_err(SubmissionError::invalid)?;
        let message = transaction.message();

        SanitizedTransaction::validate_account_locks(message, self.account_lock_limit())
            .map_err(SubmissionError::invalid)?;

        if !self.recent_blockhashes.contains(message.recent_blockhash()) {
            return Err(SubmissionError::invalid(
                "blockhash is not a recent hash issued by this rollup",
            ));
        }

        if message.instructions().is_empty() {
            return Err(SubmissionError::invalid(
                "at least one SOL transfer is required",
            ));
        }
        for (program_id, instruction) in message.program_instructions_iter() {
            if *program_id != system_program::id() {
                return Err(SubmissionError::invalid(
                    "only the SOL-transfer application is supported",
                ));
            }
            // Decoded exactly as the System program decodes it before executing.
            let instruction: SystemInstruction =
                limited_deserialize(&instruction.data, PACKET_DATA_SIZE as u64)
                    .map_err(SubmissionError::invalid)?;
            if !matches!(instruction, SystemInstruction::Transfer { .. }) {
                return Err(SubmissionError::invalid(
                    "only System Transfer instructions are supported",
                ));
            }
        }

        if self.db.transaction(transaction.signature()).is_some() {
            return Err(SubmissionError::AlreadyProcessed);
        }

        Ok(transaction)
    }

    // Agave: `Bank::get_transaction_account_lock_limit`
    // https://github.com/anza-xyz/agave/blob/v4.4.1/runtime/src/bank.rs#L3842
    fn account_lock_limit(&self) -> usize {
        if self.environment.feature_set.increase_tx_account_lock_limit {
            MAX_TX_ACCOUNT_LOCKS
        } else {
            BASE_TX_ACCOUNT_LOCKS
        }
    }

    // Phase 3, load: accounts the rollup has not seen yet are pulled from the cluster.
    //
    // Agave has no such step: a validator keeps every account in its own database. The
    // SVM reads them in phase 5 through the callback at the end of `rollupdb.rs`.
    fn load<'a>(
        &mut self,
        addresses: impl IntoIterator<Item = &'a Pubkey>,
    ) -> Result<(), SubmissionError> {
        self.db
            .pull(addresses)
            .map_err(|error| SubmissionError::ClusterUnavailable(format!("{error:#}")))
    }

    // Phase 4, budget: compute limits and the fee, derived as a validator derives them.
    //
    // Agave: `Bank::check_compute_budget_and_limits`, the same derivation
    // https://github.com/anza-xyz/agave/blob/v4.4.1/runtime/src/bank/check_transactions.rs#L148
    // Agave: `calculate_fee_details`, the same call
    // https://github.com/anza-xyz/agave/blob/v4.4.1/fee/src/lib.rs#L29
    // Agave: `calculate_priority_and_cost`, the scheduler ranks transactions by what
    // this fee pays per unit of cost. The rollup keeps arrival order.
    // https://github.com/anza-xyz/agave/blob/v4.4.1/core/src/transaction_priority.rs#L32
    fn budget(
        &self,
        transaction: &ValidTransaction,
    ) -> Result<CheckedTransactionDetails, SubmissionError> {
        // v1 carries its limits in the message config; legacy and v0 fall back to
        // the per-instruction defaults because the rollup runs no ComputeBudget program.
        let configuration = transaction
            .transaction_configuration(&self.feature_set)
            .map_err(SubmissionError::invalid)?;

        let fee_details = solana_fee::calculate_fee_details(
            transaction,
            self.environment.blockhash_lamports_per_signature,
            configuration.priority_fee_lamports,
            FeeFeatures {},
        );

        let budget = SVMTransactionExecutionBudget {
            compute_unit_limit: u64::from(configuration.compute_unit_limit),
            heap_size: configuration.updated_heap_bytes,
            ..SVMTransactionExecutionBudget::new_with_defaults(
                self.environment.feature_set.raise_cpi_nesting_limit_to_8,
            )
        };

        Ok(CheckedTransactionDetails::new(
            None,
            SVMTransactionExecutionAndFeeBudgetLimits {
                budget,
                loaded_accounts_data_size_limit: configuration.loaded_accounts_data_size_limit,
                fee_details,
            },
        ))
    }

    // Phase 5, process: the SVM loads the accounts, charges the fee and executes.
    //
    // The call below is the one a validator makes, so from there on the rollup runs
    // the validator's own code. In the order it runs:
    //
    // Agave: `Bank::do_load_and_execute_transactions`, the worker's call into the SVM
    // https://github.com/anza-xyz/agave/blob/v4.4.1/runtime/src/bank.rs#L4245
    // Agave: `TransactionBatchProcessor::load_and_execute_sanitized_transactions`
    // https://github.com/anza-xyz/agave/blob/v4.4.1/svm/src/transaction_processor.rs#L454
    // Agave: `validate_transaction_fee_payer`, charges the fee
    // https://github.com/anza-xyz/agave/blob/v4.4.1/svm/src/transaction_processor.rs#L838
    // Agave: `load_transaction`, loads the accounts through the callback
    // https://github.com/anza-xyz/agave/blob/v4.4.1/svm/src/account_loader.rs#L422
    // Agave: `execute_loaded_transaction`, creates the `TransactionContext` and the
    // `InvokeContext` that the programs of one transaction share
    // https://github.com/anza-xyz/agave/blob/v4.4.1/svm/src/transaction_processor.rs#L1097
    // Agave: `InvokeContext::process_message`, runs the instructions one by one
    // https://github.com/anza-xyz/agave/blob/v4.4.1/program-runtime/src/invoke_context.rs#L474
    // Agave: `InvokeContext::process_executable_chain`, builds a VM for the instruction
    // and calls the builtin program's entrypoint through it
    // https://github.com/anza-xyz/agave/blob/v4.4.1/program-runtime/src/invoke_context.rs#L605
    // Agave: the System program's entrypoint and its `transfer_verified`
    // https://github.com/anza-xyz/agave/blob/v4.4.1/programs/system/src/system_processor.rs#L319
    // https://github.com/anza-xyz/agave/blob/v4.4.1/programs/system/src/system_processor.rs#L216
    //
    // The System program is native code, so its VM has no memory and runs no bytecode.
    // For an on-chain program the builtin that gets called is the loader, which
    // prepares a full VM. The rollup never takes this path.
    //
    // Agave: `process_instruction_inner`, the loader's entrypoint
    // https://github.com/anza-xyz/agave/blob/v4.4.1/programs/bpf_loader/src/lib.rs#L84
    // Agave: `execute`, serializes the accounts, maps the memory and runs the program
    // https://github.com/anza-xyz/agave/blob/v4.4.1/program-runtime/src/vm.rs#L184
    // Agave: `create_vm`, takes a stack and a heap and builds the VM
    // https://github.com/anza-xyz/agave/blob/v4.4.1/program-runtime/src/vm.rs#L54
    fn process(
        &mut self,
        transaction: &ValidTransaction,
        checked: CheckedTransactionDetails,
    ) -> Result<ProcessedTransaction, SubmissionError> {
        self.environment.blockhash = *transaction.message().recent_blockhash();

        let output = self.processor.load_and_execute_sanitized_transactions(
            &self.db,
            slice::from_ref(transaction),
            vec![Ok(checked)],
            &self.environment,
            &TransactionProcessingConfig::default(),
        );

        output
            .processing_results
            .into_iter()
            .next()
            .ok_or_else(|| SubmissionError::Rejected("SVM returned no transaction result".into()))?
            .map_err(|error| SubmissionError::Rejected(error.to_string()))
    }

    // Phase 6, commit: store the account changes and the receipt as one step.
    // A failed transaction keeps only its fee payment, as it does on Mainnet.
    //
    // Agave: `TransactionRecorder::record_transactions`, a validator first records the
    // batch in its block. The rollup has no blocks.
    // https://github.com/anza-xyz/agave/blob/v4.4.1/poh/src/transaction_recorder.rs#L52
    // Agave: `Committer::commit_transactions` and `Bank::commit_transactions`
    // https://github.com/anza-xyz/agave/blob/v4.4.1/core/src/banking_stage/committer.rs#L60
    // https://github.com/anza-xyz/agave/blob/v4.4.1/runtime/src/bank.rs#L4548
    // Agave: `collect_accounts_to_store`, selects the accounts as the `match` below does
    // https://github.com/anza-xyz/agave/blob/v4.4.1/runtime/src/account_saver.rs#L54
    // Agave: `Bank::update_transaction_statuses`, remembers the signature against replay
    // https://github.com/anza-xyz/agave/blob/v4.4.1/runtime/src/bank.rs#L3699
    // Agave: `impl Drop for TransactionBatch`, releases the bank's account locks
    // https://github.com/anza-xyz/agave/blob/v4.4.1/runtime/src/transaction_batch.rs#L101
    // Agave: `complete_batch`, releases the scheduler's account locks
    // https://github.com/anza-xyz/agave/blob/v4.4.1/core/src/banking_stage/transaction_scheduler/scheduler_common.rs#L306
    fn commit(
        &mut self,
        wire_transaction: &[u8],
        transaction: &ValidTransaction,
        processed: ProcessedTransaction,
    ) -> TransactionReceipt {
        let message = transaction.message();
        let signature = *transaction.signature();
        let outcome = processed.status();

        let receipt = TransactionReceipt {
            signature: signature.to_string(),
            version: match message {
                SanitizedMessage::Legacy(_) => TransactionVersion::LEGACY,
                SanitizedMessage::V0(_) => TransactionVersion::Number(0),
                SanitizedMessage::V1(_) => TransactionVersion::Number(1),
            },
            status: match outcome {
                Ok(()) => ExecutionStatus::Executed,
                Err(_) => ExecutionStatus::Failed,
            },
            error: outcome.as_ref().err().map(ToString::to_string),
            fee: processed.fee_details().total_fee(),
            compute_units_consumed: processed.executed_units(),
            settlement: SettlementStatus::NotConfigured,
            sol_transaction: BASE64_STANDARD.encode(wire_transaction),
        };

        let accounts: Vec<(Pubkey, AccountSharedData)> = match processed {
            // The accounts the SVM itself stores after a successful transaction.
            ProcessedTransaction::Executed(executed) if outcome.is_ok() => executed
                .loaded_transaction
                .accounts
                .into_iter()
                .zip(executed.loaded_transaction.touched_flags)
                .enumerate()
                .filter(|(index, (_, touched))| {
                    *touched
                        && message.is_writable(*index)
                        && (!message.is_invoked(*index) || message.is_instruction_account(*index))
                })
                .map(|(_, (account, _))| account)
                .collect(),
            // Never persist failed execution accounts; the SVM leaves partial writes in them.
            ProcessedTransaction::Executed(executed) => {
                rollback_accounts(&executed.loaded_transaction.rollback_accounts)
            }
            ProcessedTransaction::FeesOnly(fees_only) => {
                rollback_accounts(&fees_only.rollback_accounts)
            }
            // A no-op lands without a fee; Mainnet has not activated them yet.
            ProcessedTransaction::NoOp(_) => Vec::new(),
        };

        self.db.commit(signature, receipt.clone(), accounts);

        receipt
    }
}

// Agave: `RollbackAccounts`
// https://github.com/anza-xyz/agave/blob/v4.4.1/svm/src/rollback_accounts.rs#L12

/// The fee payer after paying the fee, with every other change of the transaction undone.
fn rollback_accounts(
    rollback_accounts: &solana_svm::rollback_accounts::RollbackAccounts,
) -> Vec<(Pubkey, AccountSharedData)> {
    rollback_accounts
        .into_iter()
        .map(|(address, account)| (*address, account.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use anyhow::{ensure, Context};
    use reqwest::blocking::Client;
    use serde_json::{json, Value};

    use super::*;

    const MAINNET_RPC: &str = "https://api.mainnet-beta.solana.com";

    fn rpc(client: &Client, method: &str, params: Value) -> Result<Value> {
        let request = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let mut response: Value = client
            .post(MAINNET_RPC)
            .json(&request)
            .send()?
            .error_for_status()?
            .json()?;

        ensure!(
            response["error"].is_null(),
            "{method}: {}",
            response["error"]
        );
        Ok(response["result"].take())
    }

    /// Run with `cargo xtask mainnet`. It needs the public Mainnet RPC, so plain
    /// `cargo test` skips it.
    #[test]
    #[ignore = "queries the public Solana Mainnet RPC"]
    fn mainnet_snapshot_is_current() -> Result<()> {
        let client = Client::new();
        let features: Vec<Pubkey> = feature::FEATURE_NAMES.keys().copied().collect();

        // A feature is active once its account holds an activation slot.
        let mut live_inactive = HashSet::new();
        for chunk in features.chunks(100) {
            let addresses: Vec<String> = chunk.iter().map(Pubkey::to_string).collect();
            let accounts = rpc(
                &client,
                "getMultipleAccounts",
                json!([addresses, {"encoding": "base64"}]),
            )?;
            let accounts = accounts["value"].as_array().context("feature accounts")?;

            for (feature_id, account) in chunk.iter().zip(accounts) {
                let data = account["data"][0].as_str().unwrap_or_default();
                let data = BASE64_STANDARD.decode(data)?;
                if data.first() != Some(&1) {
                    live_inactive.insert(*feature_id);
                }
            }
        }

        let snapshot: HashSet<Pubkey> = MAINNET_INACTIVE_FEATURES.into_iter().collect();
        let describe = |ids: Vec<&Pubkey>| -> Vec<String> {
            ids.into_iter()
                .map(|id| format!("{id} ({})", feature::FEATURE_NAMES[id]))
                .collect()
        };
        let activated = describe(snapshot.difference(&live_inactive).collect());
        let missing = describe(live_inactive.difference(&snapshot).collect());
        ensure!(
            activated.is_empty() && missing.is_empty(),
            "MAINNET_INACTIVE_FEATURES is stale.\nNow active on Mainnet: {activated:#?}\n\
             Inactive on Mainnet but not listed: {missing:#?}"
        );

        let minimum_balance = rpc(&client, "getMinimumBalanceForRentExemption", json!([0]))?;
        let rent = Rent::with_lamports_per_byte(LAMPORTS_PER_BYTE);
        ensure!(
            minimum_balance.as_u64() == Some(rent.minimum_balance(0)),
            "LAMPORTS_PER_BYTE is stale: Mainnet's minimum balance is {minimum_balance}, \
             the rollup's is {}",
            rent.minimum_balance(0)
        );

        println!(
            "Mainnet snapshot is current: {} inactive features, {LAMPORTS_PER_BYTE} lamports per byte",
            snapshot.len()
        );
        Ok(())
    }
}
