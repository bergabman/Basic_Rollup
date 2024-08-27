use std::collections::HashMap;
use anyhow::Result;
use solana_sdk::{keccak::{Hash, Hasher}, transaction::Transaction};
use serde::{Serialize, Deserialize};
use bincode;
// use serde_json;

#[derive(Serialize, Deserialize, Debug)]
struct RollupTransaction {
    sender: String,
    sol_transaction: Transaction
}

#[derive(Serialize, Deserialize, Debug)]
pub struct GetTransaction {
    pub get_tx: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    
    let client = reqwest::Client::new();

    let test_response = client.get("http://127.0.0.1:8080")
        .send()
        .await?
        .json::<HashMap<String, String>>()
        .await?;

    println!("{test_response:#?}");

    let rtx = RollupTransaction {
        sender: "Me".into(),
        sol_transaction: Transaction::default(),
    };

    // let serialized_rollup_transaction = serde_json::to_string(&rtx)?;

    let submit_transaction = client
        .post("http://127.0.0.1:8080/submit_transaction")
        .json(&rtx)
        .send()
        .await?;
        // .json()
        // .await?;

    println!("{submit_transaction:#?}");
    let mut hasher = Hasher::default();
    hasher.hash(bincode::serialize(&rtx.sol_transaction).unwrap().as_slice());
    
    println!("{:#?}", hasher.clone().result());

    let tx_resp = client
        .post("http://127.0.0.1:8080/get_transaction")
        .json(&HashMap::from([("get_tx", hasher.result().to_string())]))
        .send()
        .await?
        .json::<HashMap<String, String>>()
        .await?;

    println!("{tx_resp:#?}");

    
    Ok(())
}

