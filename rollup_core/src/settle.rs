use anyhow::Result;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{hash::Hash, transaction::Transaction};

// Settle the state on solana, called by sequencer
pub async fn settle_state(proof: Hash) -> Result<String> {
    let rpc_client = RpcClient::new("https://api.devnet.solana.com".into());

    // Create proof transaction, calling the right function in the contract
    let transaction = Transaction::default();

    // Send transaction to contract on chain
    let settle_tx_hash = rpc_client
        .send_and_confirm_transaction(&transaction)
        .await?;
    Ok(settle_tx_hash.to_string())
}
