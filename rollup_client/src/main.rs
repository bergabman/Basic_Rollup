use anyhow::Result;
use bincode;
use serde::{Deserialize, Serialize};
use solana_client::nonblocking::rpc_client::{self, RpcClient};
use solana_sdk::{
    instruction::Instruction,
    hash::{Hash, Hasher},
    native_token::LAMPORTS_PER_SOL,
    signature::Signature,
    signer::{self, Signer},
    system_instruction, system_program,
    transaction::Transaction,
};
use solana_transaction_status::UiTransactionEncoding::{self, Binary};
use core::hash;
use std::{collections::HashMap, ops::Div, str::FromStr};
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

#[tokio::main]
async fn main() -> Result<()> {
    let path = "/home/izomana/adv-svm/Basic_Rollup_fork/rollup_client/mykey_1.json";
    let path2 = "/home/izomana/adv-svm/Basic_Rollup_fork/rollup_client/testkey.json";
    let path3 = "/home/izomana/adv-svm/Basic_Rollup_fork/rollup_client/owner.json";
    let keypair = signer::keypair::read_keypair_file(path.to_string()).unwrap();
    let keypair2 = signer::keypair::read_keypair_file(path2.to_string()).unwrap();
    let rpc_client = RpcClient::new("https://api.devnet.solana.com".into());

    let ix =
        system_instruction::transfer(&keypair2.pubkey(), &keypair.pubkey(), 1 * (LAMPORTS_PER_SOL/4));
    let tx = Transaction::new_signed_with_payer(
        &[ix],
        Some(&keypair2.pubkey()),
        &[&keypair2],
        rpc_client.get_latest_blockhash().await.unwrap(),
    );

    // let sig = Signature::from_str("3ENa2e9TG6stDNkUZkRcC2Gf5saNMUFhpptQiNg56nGJ9eRBgSJpZBi7WLP5ev7aggG1JAXQWzBk8Xfkjcx1YCM2").unwrap();
    // let tx = rpc_client.get_transaction(&sig, UiTransactionEncoding::Binary).await.unwrap();
    let client = reqwest::Client::new();

    // let tx_encoded: Transaction = tx.try_into().unwrap();

    let test_response = client
        .get("http://127.0.0.1:8080")
        .send()
        .await?
        .json::<HashMap<String, String>>()
        .await?;

    println!("{test_response:#?}");

    let rtx = RollupTransaction {
        sender: "Me".into(),
        sol_transaction: tx,
    };

    // let serialized_rollup_transaction = serde_json::to_string(&rtx)?;

    //UNCOMMENT
    // let submit_transaction = client
    //     .post("http://127.0.0.1:8080/submit_transaction")
    //     .json(&rtx)
    //     .send()
    //     .await?;
    // // .json()
    // // .await?;

    // println!("{submit_transaction:#?}");
    // let mut hasher = Hasher::default();
    // hasher.hash(bincode::serialize(&rtx.sol_transaction).unwrap().as_slice());

    // println!("{:#?}", hasher.clone().result());

    // let tx_resp = client
    //     .post("http://127.0.0.1:8080/get_transaction")
    //     .json(&GetTransaction{get_tx: rtx.sol_transaction.message.hash().to_string()})
    //     .send()
    //     .await?;
    //     // .json::<HashMap<String, String>>()
    //     // .await?;

    // println!("{tx_resp:#?}");

    // let amounts: Vec<i32> = vec![4, -2, 3, -5, 1, -4, 2, -1, 3, -1];
    let amounts: Vec<(String, String, i32)> = vec![
        (path.to_string(), path2.to_string(), 5),
        (path3.to_string(), path.to_string(), -3),
        (path2.to_string(), path3.to_string(), 8),
        (path.to_string(), path3.to_string(), -7),
        (path2.to_string(), path.to_string(), 4),
        (path3.to_string(), path2.to_string(), -6),
        (path.to_string(), path2.to_string(), 9),
        (path2.to_string(), path3.to_string(), -2),
        (path3.to_string(), path.to_string(), 1),
        (path.to_string(), path3.to_string(), -4),
    ];
    let mut txs: Vec<Transaction> = vec![];
    for amt in amounts {
        if amt.2 > 0 {
            txs.push(gen_transfer_tx(amt.0, amt.1, amt.2 as u64).await);
        } else {
            txs.push(gen_transfer_tx(amt.1, amt.0, amt.2.abs() as u64).await);
        }
    }

    for tx in txs {
        let rtx = RollupTransaction {
            sender: "Me".into(),
            sol_transaction: tx
        };

        let submission = client
            .post("http://127.0.0.1:8080/submit_transaction")
            .json(&rtx)
            .send()
            .await?;
        
        println!("Submission {submission:#?}");
    }

    println!("KP: {}", keypair.pubkey());
    println!("KP2: {}", keypair2.pubkey());

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