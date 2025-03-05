//! A helper to initialize Solana SVM API's `TransactionBatchProcessor`.

use {
    solana_bpf_loader_program::syscalls::create_program_runtime_environment_v1,
    solana_compute_budget::compute_budget::ComputeBudget,
    solana_program_runtime::loaded_programs::{BlockRelation, ForkGraph, ProgramCacheEntry},
    solana_sdk::{clock::Slot, feature_set::FeatureSet, pubkey::Pubkey, transaction},
    solana_svm::{
        account_loader::CheckedTransactionDetails,
        transaction_processing_callback::TransactionProcessingCallback,
        transaction_processor::TransactionBatchProcessor,
    },
    solana_system_program::system_processor,
    std::sync::{Arc, RwLock},
    std::collections::HashSet,
};

/// In order to use the `TransactionBatchProcessor`, another trait - Solana
/// Program Runtime's `ForkGraph` - must be implemented, to tell the batch
/// processor how to work across forks.
///
/// Since PayTube doesn't use slots or forks, this implementation is mocked.
pub(crate) struct RollupForkGraph {}

impl ForkGraph for RollupForkGraph {
    fn relationship(&self, _a: Slot, _b: Slot) -> BlockRelation {
        BlockRelation::Unknown
    }
}

/// This function encapsulates some initial setup required to tweak the
/// `TransactionBatchProcessor` for use within PayTube.
///
/// We're simply configuring the mocked fork graph on the SVM API's program
/// cache, then adding the System program to the processor's builtins.
pub(crate) fn create_transaction_batch_processor<CB: TransactionProcessingCallback>(
    callbacks: &CB,
    feature_set: &FeatureSet,
    compute_budget: &ComputeBudget,
    fork_graph: Arc<RwLock<RollupForkGraph>>,
) -> TransactionBatchProcessor<RollupForkGraph> {
    let processor = TransactionBatchProcessor::<RollupForkGraph>::new(
        /* slot */ 1,
        /* epoch */ 1, 
        Arc::downgrade(&fork_graph),
        Some(Arc::new(
            create_program_runtime_environment_v1(feature_set, compute_budget, false, false)
                .unwrap(),
        )),
        None,
    );

    processor.program_cache.write().unwrap().set_fork_graph(Arc::downgrade(&fork_graph));

    processor.prepare_program_cache_for_upcoming_feature_set(
        callbacks, feature_set, compute_budget, 1, 50
    );

    // Add system program
    processor.add_builtin(
        callbacks,
        solana_system_program::id(),
        "system_program", 
        ProgramCacheEntry::new_builtin(0, b"system_program".len(), system_processor::Entrypoint::vm),
    );

    processor
}

/// This function is also a mock. In the Agave validator, the bank pre-checks
/// transactions before providing them to the SVM API. We mock this step in
/// PayTube, since we don't need to perform such pre-checks.
pub(crate) fn get_transaction_check_results(
    len: usize,
    lamports_per_signature: u64,
) -> Vec<transaction::Result<CheckedTransactionDetails>> {
    vec![
        transaction::Result::Ok(CheckedTransactionDetails::new(
            None,
            lamports_per_signature
        ));
        len
    ]
}
