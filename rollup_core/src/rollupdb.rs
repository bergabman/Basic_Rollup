use async_channel::{Receiver, Sender};
use serde::{Deserialize, Serialize};
use solana_sdk::{
    account::AccountSharedData, keccak::Hash, pubkey::Pubkey, transaction::Transaction,
};

use crossbeam::channel::{Receiver as CBReceiver, Sender as CBSender};
use std::{
    collections::{HashMap, HashSet},
    default,
};

use crate::frontend::FrontendMessage;

#[derive(Serialize, Deserialize)]
pub struct RollupDBMessage {
    pub lock_accounts: Option<Vec<Pubkey>>,
    pub add_processed_transaction: Option<Transaction>,
    pub frontend_get_tx: Option<Hash>,
    pub add_settle_proof: Option<String>,
}

#[derive(Serialize, Debug, Default)]
pub struct RollupDB {
    accounts_db: HashMap<Pubkey, AccountSharedData>,
    locked_accounts: HashMap<Pubkey, AccountSharedData>,
    transactions: HashMap<Hash, Transaction>,
}

impl RollupDB {
    pub async fn run(
        rollup_db_receiver: CBReceiver<RollupDBMessage>,
        frontend_sender: Sender<FrontendMessage>,
    ) {
        let mut db = RollupDB {
            accounts_db: HashMap::new(),
            locked_accounts: HashMap::new(),
            transactions: HashMap::new(),
        };

        while let Ok(message) = rollup_db_receiver.recv() {
            if let Some(accounts_to_lock) = message.lock_accounts {
                // Lock accounts, by removing them from the accounts_db hashmap, and adding them to locked accounts
                let _ = accounts_to_lock.iter().map(|pubkey| {
                    db.locked_accounts
                        .insert(pubkey.clone(), db.accounts_db.remove(pubkey).unwrap())
                });
            } else if let Some(get_this_hash_tx) = message.frontend_get_tx {
                let req_tx = db.transactions.get(&get_this_hash_tx).unwrap();

                frontend_sender
                    .send(FrontendMessage {
                        transaction: Some(req_tx.clone()),
                        get_tx: None,
                    })
                    .await
                    .unwrap();
            } else if let Some(tx) = message.add_processed_transaction {
            }
        }
    }
}
