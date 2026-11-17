use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
    str::FromStr,
    time::Duration,
};

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use solana_account::{AccountSharedData, ReadableAccount, WritableAccount};
use solana_pubkey::Pubkey;
use solana_rpc_client::rpc_client::RpcClient;
use solana_sdk_ids::{native_loader, system_program};
use solana_signature::Signature;
use solana_svm_callback::{InvokeContextCallback, TransactionProcessingCallback};
use solana_transaction::versioned::TransactionVersion;

use crate::settle::SettlementStatus;

/// How long one cluster request may take before the submission is refused.
const CLUSTER_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Genesis {
    accounts: Vec<GenesisAccount>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GenesisAccount {
    address: String,
    lamports: u64,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TransactionReceipt {
    pub signature: String,
    /// `"legacy"`, `0` or `1`, as Solana RPC reports it.
    pub version: TransactionVersion,
    pub status: ExecutionStatus,
    /// Why execution failed; absent on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Lamports charged to the fee payer, including any priority fee.
    pub fee: u64,
    pub compute_units_consumed: u64,
    pub settlement: SettlementStatus,
    /// Base64 of the serialized transaction, as it was submitted.
    pub sol_transaction: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExecutionStatus {
    Executed,
    /// Landed and paid its fee, but execution failed and its changes were undone.
    Failed,
}

#[derive(Serialize)]
pub(crate) struct AccountBalance {
    pub address: String,
    pub lamports: u64,
}

/// The sequencer exclusively owns the in-memory application state and receipts.
pub(crate) struct RollupDB {
    accounts: HashMap<Pubkey, AccountSharedData>,
    transactions: HashMap<Signature, TransactionReceipt>,
    /// The Solana cluster that accounts are pulled from on first use, when configured.
    cluster: Option<RpcClient>,
    /// Addresses the rollup has taken ownership of. They are never pulled again, so a
    /// cluster balance cannot come back after the rollup spent or removed it.
    resolved: HashSet<Pubkey>,
}

impl RollupDB {
    /// Starts with the System program alone. Every other account comes from genesis or
    /// from the cluster at `cluster_url`.
    pub fn new(cluster_url: Option<&str>) -> Self {
        let mut system = AccountSharedData::empty(1, &native_loader::id());
        system.set_executable(true);

        Self {
            accounts: HashMap::from([(system_program::id(), system)]),
            transactions: HashMap::new(),
            cluster: cluster_url.map(|url| RpcClient::new_with_timeout(url, CLUSTER_TIMEOUT)),
            resolved: HashSet::from([system_program::id()]),
        }
    }

    // Agave: `Bank::process_genesis_config`
    // https://github.com/anza-xyz/agave/blob/v4.4.1/runtime/src/bank.rs#L3356

    /// Seeds the account state the way a validator builds its first bank from
    /// `genesis.bin`. A genesis account takes precedence over the cluster's copy.
    pub fn load_genesis(&mut self, path: &Path) -> Result<()> {
        let contents = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let genesis: Genesis =
            serde_json::from_slice(&contents).context("parsing genesis accounts")?;
        ensure!(
            !genesis.accounts.is_empty(),
            "genesis must contain at least one funded demo account"
        );

        let mut total_lamports = 0_u64;
        for account in genesis.accounts {
            let address = Pubkey::from_str(&account.address).context("invalid genesis address")?;
            ensure!(
                address != system_program::id(),
                "genesis cannot replace the System program"
            );
            total_lamports = total_lamports
                .checked_add(account.lamports)
                .context("genesis lamport supply exceeds u64")?;
            ensure!(
                self.resolved.insert(address),
                "duplicate genesis account {address}"
            );
            let data = AccountSharedData::empty(account.lamports, &system_program::id());
            self.accounts.insert(address, data);
        }

        Ok(())
    }

    /// Copies the accounts the rollup has not seen yet from the cluster, as they are at
    /// this moment. From then on the rollup reads and changes only its own copy.
    pub fn pull<'a>(&mut self, addresses: impl IntoIterator<Item = &'a Pubkey>) -> Result<()> {
        let Some(cluster) = &self.cluster else {
            return Ok(());
        };
        let unseen: Vec<Pubkey> = addresses
            .into_iter()
            .filter(|address| !self.resolved.contains(*address))
            .copied()
            .collect();
        if unseen.is_empty() {
            return Ok(());
        }

        log::info!("pulling {} account(s) from the cluster", unseen.len());
        // One request is enough: a transaction locks at most 64 accounts, the RPC takes 100.
        let pulled = cluster
            .get_multiple_accounts(&unseen)
            .context("pulling accounts from the cluster")?;
        for (address, account) in unseen.into_iter().zip(pulled) {
            // An address without an account on the cluster is resolved too: it is new.
            if let Some(account) = account {
                self.accounts.insert(address, account.into());
            }
            self.resolved.insert(address);
        }

        Ok(())
    }

    pub fn transaction(&self, signature: &Signature) -> Option<TransactionReceipt> {
        self.transactions.get(signature).cloned()
    }

    pub fn balance(&self, address: &Pubkey) -> Option<AccountBalance> {
        self.accounts.get(address).map(|account| AccountBalance {
            address: address.to_string(),
            lamports: account.lamports(),
        })
    }

    // Agave: `Accounts::store_accounts`, writes a committed batch to the accounts database
    // https://github.com/anza-xyz/agave/blob/v4.4.1/accounts-db/src/accounts.rs#L509
    pub fn commit(
        &mut self,
        signature: Signature,
        receipt: TransactionReceipt,
        accounts: impl IntoIterator<Item = (Pubkey, AccountSharedData)>,
    ) {
        for (address, account) in accounts {
            // An account left without lamports no longer exists, as on Mainnet.
            if account.lamports() == 0 {
                self.accounts.remove(&address);
            } else {
                self.accounts.insert(address, account);
            }
        }

        self.transactions.insert(signature, receipt);
    }
}

// The SVM asks for each account of a transaction through this callback.
// Agave: `AccountLoader::do_load`, the caller inside the SVM
// https://github.com/anza-xyz/agave/blob/v4.4.1/svm/src/account_loader.rs#L279
// Agave: `impl TransactionProcessingCallback for Bank`, the validator's answer
// https://github.com/anza-xyz/agave/blob/v4.4.1/runtime/src/bank.rs#L6987
impl TransactionProcessingCallback for RollupDB {
    fn get_account_shared_data(&self, pubkey: &Pubkey) -> Option<AccountSharedData> {
        self.accounts.get(pubkey).cloned()
    }
}

impl InvokeContextCallback for RollupDB {}
