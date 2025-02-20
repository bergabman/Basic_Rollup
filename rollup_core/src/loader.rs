//! PayTube's "account loader" component, which provides the SVM API with the
//! ability to load accounts for PayTube channels.
//!
//! The account loader is a simple example of an RPC client that can first load
//! an account from the base chain, then cache it locally within the protocol
//! for the duration of the channel.

use {
    log::info, solana_client::rpc_client::RpcClient, solana_sdk::{
        account::{AccountSharedData, ReadableAccount},
        pubkey::Pubkey,
    }, solana_svm::transaction_processing_callback::TransactionProcessingCallback, std::{collections::HashMap, sync::RwLock},
    std::ops::IndexMut,
};

// impl IndexMut for HashMap<K, V> {
//     fn index_mut(&mut self, index: Idx) -> &mut Self::Output {
        
//     }
// }
/// An account loading mechanism to hoist accounts from the base chain up to
/// an active PayTube channel.
///
/// Employs a simple cache mechanism to ensure accounts are only loaded once.
pub struct RollupAccountLoader<'a> {
    pub cache: RwLock<HashMap<Pubkey, AccountSharedData>>,
    pub rpc_client: &'a RpcClient,
}

impl<'a> RollupAccountLoader<'a> {
    pub fn new(rpc_client: &'a RpcClient) -> Self {
        Self {
            cache: RwLock::new(HashMap::new()),
            rpc_client,
        }
    }

    pub fn add_account(&mut self, pubkey: Pubkey, modified_or_new_account: AccountSharedData) {
        let mut map = self.cache.write().unwrap();
        let res = map.contains_key(&pubkey);
        if res == false {
            map.insert(pubkey, modified_or_new_account);
            log::info!("newone: {:?}", map);
        } else { // PROBLEM HERE!!! SOMEHOW DON'T FIND THIS ELSE
        
            map.insert(pubkey, modified_or_new_account);
            // map.entry(pubkey)
            // .and_modify(|account| {
            //     log::info!("Updating existing account.");
            //     *account = modified_or_new_account.clone(); // Replace existing account
            // });
            log::info!("oldone: {:?}", map);
            
        }
    //     let mut cache = self.cache.write().unwrap(); // Get a write lock once

    // if let Some(account) = cache.get_mut(&pubkey) {
    //     log::info!("Updating existing account");
    //     *account = modified_or_new_account; // Overwrite existing entry
    // } else {
    //     log::info!("Adding new account");
    //     cache.insert(pubkey, modified_or_new_account); // Insert new entry
    // }


        // let mut cache = self.cache.write().unwrap();


        // cache.entry(pubkey)
        // .and_modify(|account| {
        //     log::info!("Updating existing account.");
        //     *account = modified_or_new_account.clone(); // Replace existing account
        // })
        // .or_insert_with(|| {
        //     log::info!("Inserting new account.");
        //     modified_or_new_account
        // });
        // if let Some(account) = self.cache.read().unwrap().get(&pubkey) {
        //     log::info!("it is alright");
        //     cache.entry(pubkey).and_modify(
        //         //     |account| {
        //         //     let data = account.data();
        //         //     account.set_data_from_slice(&data)
        //         // }
        //         |account| *account = modified_or_new_account.clone()
        //         );
        // }
        // else {
        //     self.cache.write().unwrap().insert(pubkey, modified_or_new_account);
        //     log::info!("fucking problem");
        // } 
        // log::info!("cache: {:?}", self.cache.read().unwrap());
        // log::info!("entryyy: {:?}", self.cache.read().unwrap().get(&pubkey));
        
    }
}

/// Implementation of the SVM API's `TransactionProcessingCallback` interface.
///
/// The SVM API requires this plugin be provided to provide the SVM with the
/// ability to load accounts.
///
/// In the Agave validator, this implementation is Bank, powered by AccountsDB.
impl TransactionProcessingCallback for RollupAccountLoader<'_> {
    fn get_account_shared_data(&self, pubkey: &Pubkey) -> Option<AccountSharedData> {
        if let Some(account) = self.cache.read().unwrap().get(pubkey) {
            log::info!("the account was loaded from the database");
            return Some(account.clone());
        }
        else {
            None
        }

        // let account: AccountSharedData = self.rpc_client.get_account(pubkey).ok()?.into();
        // self.cache.write().unwrap().insert(*pubkey, account.clone());
        // log::info!("the account was loaded from the rpcclient");
        // Some(account)
    }

    fn account_matches_owners(&self, account: &Pubkey, owners: &[Pubkey]) -> Option<usize> {
        self.get_account_shared_data(account)
            .and_then(|account| owners.iter().position(|key| account.owner().eq(key)))
    }
}
