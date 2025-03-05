use anyhow::Result;
use bincode;
use serde::{Deserialize, Serialize};
use solana_client::nonblocking::rpc_client::{self, RpcClient};
use solana_sdk::{
    instruction::Instruction,
    hash::{Hash, Hasher},
    native_token::LAMPORTS_PER_SOL,
    signature::{Keypair, Signer},
    system_instruction, system_program,
    transaction::Transaction,
    pubkey::Pubkey,
};
use solana_transaction_status::UiTransactionEncoding::{self, Binary};
use std::{collections::HashMap, str::FromStr, time::Duration, fs};
// use serde_json;

#[derive(Serialize, Deserialize, Debug)]
struct RollupTransaction {
    sender: String,
    sol_transaction: Transaction,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct GetTransaction {
    pub get_tx: String,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "status", content = "data")]
pub enum TransactionResponse {
    Success { message: String },
    Error { message: String },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load existing keypairs from files
    let sender = Keypair::from_bytes(&fs::read("keys/sender.json")?)?;
    let receiver = Keypair::from_bytes(&fs::read("keys/receiver.json")?)?;
    println!("rec: {:?}", receiver.pubkey());
    
    // Connect to devnet
    let rpc_client = RpcClient::new("https://api.devnet.solana.com".to_string());
    
    // Print initial balances
    let sender_balance = rpc_client.get_balance(&sender.pubkey()).await?;
    let receiver_balance = rpc_client.get_balance(&receiver.pubkey()).await?;
    println!("Initial Sender {} balance: {} SOL", sender.pubkey(), sender_balance as f64 / 1_000_000_000.0);
    println!("Initial Receiver {} balance: {} SOL", receiver.pubkey(), receiver_balance as f64 / 1_000_000_000.0);

    // Initialize delegation service for sender
    println!("\nInitializing delegation service for sender...");
    let client = reqwest::Client::new();
    let response = client
        .post("http://127.0.0.1:8080/init_delegation_service")
        .body(sender.to_bytes().to_vec())
        .send()
        .await?;
    println!("Sender delegation service init response: {:?}", response.text().await?);

    // Wait for initialization
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Initialize delegation service for receiver
    println!("\nInitializing delegation service for receiver...");
    let response = client
        .post("http://127.0.0.1:8080/add_delegation_signer")
        .body(receiver.to_bytes().to_vec())
        .send()
        .await?;
    println!("Receiver delegation service init response: {:?}", response.text().await?);

    // Wait for initialization
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Create test transactions
    let amounts = vec![5, -3, 9, -10, 1, -10, 4, -3, 9, -6];
    let mut txs = Vec::new();
    
    for amount in amounts {
        let (from, to, lamports) = if amount > 0 {
            (&sender, &receiver, amount as u64)
        } else {
            (&receiver, &sender, (-amount) as u64)
        };

        let ix = system_instruction::transfer(
            &from.pubkey(),
            &to.pubkey(),
            lamports * (LAMPORTS_PER_SOL / 10)
        );

        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&from.pubkey()),
            &[from],
            rpc_client.get_latest_blockhash().await?,
        );

        txs.push(tx);
    }

    // Submit transactions
    println!("\nSubmitting transactions...");
    for (i, tx) in txs.into_iter().enumerate() {
        let rtx = RollupTransaction {
            sender: sender.pubkey().to_string(),
            sol_transaction: tx,
        };

        let response = client
            .post("http://127.0.0.1:8080/submit_transaction")
            .json(&rtx)
            .send()
            .await?;
            
        println!("Transaction {} response: {:?}", i + 1, response.text().await?);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    // Print final balances
    let sender_balance = rpc_client.get_balance(&sender.pubkey()).await?;
    let receiver_balance = rpc_client.get_balance(&receiver.pubkey()).await?;
    println!("\nFinal Sender {} balance: {} SOL", sender.pubkey(), sender_balance as f64 / 1_000_000_000.0);
    println!("Final Receiver {} balance: {} SOL", receiver.pubkey(), receiver_balance as f64 / 1_000_000_000.0);
    
    Ok(())
}

async fn gen_transfer_tx(path1: String, path2: String, amount: u64) -> Transaction {
    println!("Amount: {amount}");
    let keypair = signer::keypair::read_keypair_file(path1.to_string()).unwrap();
    let keypair2 = signer::keypair::read_keypair_file(path2.to_string()).unwrap();
    let rpc_client = RpcClient::new("https://api.devnet.solana.com".into());

    let ix =
        system_instruction::transfer(&keypair2.pubkey(), &keypair.pubkey(), amount * (LAMPORTS_PER_SOL / 10));
    Transaction::new_signed_with_payer(
        &[ix],
        Some(&keypair2.pubkey()),
        &[&keypair2],
        rpc_client.get_latest_blockhash().await.unwrap(),
    )
}