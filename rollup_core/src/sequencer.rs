use std::{collections::HashMap, sync::{Arc, RwLock}};

use async_channel::{Receiver, Sender};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{account::{AccountSharedData, ReadableAccount}, clock::Slot, feature_set::FeatureSet, fee::FeeStructure, pubkey::Pubkey, rent_collector::RentCollector, transaction::Transaction};
use solana_compute_budget::compute_budget::ComputeBudget;
use solana_program_runtime::loaded_programs::{BlockRelation, ForkGraph, LoadProgramMetrics, ProgramCacheEntry};
use anyhow::{anyhow, Result};
use solana_svm::transaction_processing_callback::TransactionProcessingCallback;

use crate::{rollupdb::RollupDBMessage, settle::settle_state};

pub async fn run(
    sequencer_receiver_channel: Receiver<Transaction>,
    rollupdb_sender: Sender<RollupDBMessage>,
) -> Result<()> {
    let mut tx_counter = 0u32;
    while let Ok(transaction) = sequencer_receiver_channel.recv().await {
        let accounts_to_lock = transaction.message.account_keys.clone();
        tx_counter += 1;
        // lock accounts in rollupdb to keep paralell execution possible, just like on solana
        rollupdb_sender
            .send(RollupDBMessage {
                lock_accounts: Some(accounts_to_lock),
                frontend_get_tx: None,
                add_settle_proof: None,
                add_processed_transaction: None,
            })
            .await
            .map_err(|_| anyhow!("failed to send message to rollupdb"))?;

        // Verify ransaction signatures, integrity

        // Process transaction

        let compute_budget = ComputeBudget::default();
        let feature_set = FeatureSet::all_enabled();
        let fee_structure = FeeStructure::default();
        let lamports_per_signature = fee_structure.lamports_per_signature;
        let rent_collector = RentCollector::default();

        // Solana runtime.
        let fork_graph = Arc::new(RwLock::new(SequencerForkGraph {}));
       
        // create transaction processor, add accounts and programs, builtins, 






        // Send processed transaction to db for storage and availability
        rollupdb_sender
            .send(RollupDBMessage {
                lock_accounts: None,
                add_processed_transaction: Some(transaction),
                frontend_get_tx: None,
                add_settle_proof: None,
            })
            .await
            .unwrap();

        // Call settle if transaction amount since last settle hits 10
        if tx_counter >= 10 {
            // Lock db to avoid state changes during settlement

            // Prepare root hash, or your own proof to send to chain

            // Send proof to chain

            let _settle_tx_hash = settle_state("proof".into()).await?;
            tx_counter = 0u32;
        }
    }

    Ok(())
}


/// In order to use the `TransactionBatchProcessor`, another trait - Solana
/// Program Runtime's `ForkGraph` - must be implemented, to tell the batch
/// processor how to work across forks.
///
/// Since our rollup doesn't use slots or forks, this implementation is mocked.
pub(crate) struct SequencerForkGraph {}

impl ForkGraph for SequencerForkGraph {
    fn relationship(&self, _a: Slot, _b: Slot) -> BlockRelation {
        BlockRelation::Unknown
    }
}
pub struct SequencerAccountLoader<'a> {
    cache: RwLock<HashMap<Pubkey, AccountSharedData>>,
    rpc_client: &'a RpcClient,
}

impl<'a> SequencerAccountLoader<'a> {
    pub fn new(rpc_client: &'a RpcClient) -> Self {
        Self {
            cache: RwLock::new(HashMap::new()),
            rpc_client,
        }
    }
}

/// Implementation of the SVM API's `TransactionProcessingCallback` interface.
///
/// The SVM API requires this plugin be provided to provide the SVM with the
/// ability to load accounts.
///
/// In the Agave validator, this implementation is Bank, powered by AccountsDB.
impl TransactionProcessingCallback for SequencerAccountLoader<'_> {
    fn get_account_shared_data(&self, pubkey: &Pubkey) -> Option<AccountSharedData> {





        if let Some(account) = self.cache.read().unwrap().get(pubkey) {
            return Some(account.clone());
        }

        let account: AccountSharedData = self.rpc_client.get_account(pubkey).ok()?.into();
        self.cache.write().unwrap().insert(*pubkey, account.clone());

        Some(account)
    }

    fn account_matches_owners(&self, account: &Pubkey, owners: &[Pubkey]) -> Option<usize> {
        self.get_account_shared_data(account)
            .and_then(|account| owners.iter().position(|key| account.owner().eq(key)))
    }
}
