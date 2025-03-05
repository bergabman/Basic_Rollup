use {
    crate::delegation::{create_delegation_instruction, create_withdrawal_instruction, find_delegation_pda, DelegatedAccount, create_topup_instruction}, anyhow::{anyhow, Result}, borsh::BorshDeserialize, log, solana_client::rpc_client::RpcClient, solana_sdk::{
        account::{AccountSharedData, ReadableAccount}, message::Message, pubkey::Pubkey, signature::Keypair, signer::Signer, transaction::Transaction
    }, std::collections::HashMap
};

pub struct DelegationService {
    rpc_client: RpcClient,
    pda_cache: HashMap<Pubkey, AccountSharedData>,
    signers: HashMap<Pubkey, Keypair>,  // Store multiple signers
}

impl DelegationService {
    pub fn new(rpc_url: &str, initial_signer: Keypair) -> Self {
        let mut signers = HashMap::new();
        signers.insert(initial_signer.pubkey(), initial_signer);
        
        Self {
            rpc_client: RpcClient::new(rpc_url.to_string()),
            pda_cache: HashMap::new(),
            signers,
        }
    }

    pub fn add_signer(&mut self, signer: Keypair) {
        self.signers.insert(signer.pubkey(), signer);
    }

    pub fn get_or_fetch_pda(&mut self, user: &Pubkey) -> Result<Option<(Pubkey, DelegatedAccount)>> {
        let (pda, _) = find_delegation_pda(user);
        
        // Always try fetching from chain first to be sure
        match self.rpc_client.get_account(&pda) {
            Ok(account) => {
                log::info!(
                    "Found account for PDA: {}, data length: {}, owner: {}", 
                    pda,
                    account.data().len(),
                    account.owner()
                );
                // Skip the 8-byte discriminator when deserializing
                if account.data().len() > 8 {
                    if let Ok(delegation) = DelegatedAccount::try_from_slice(&account.data()[8..]) {
                        self.pda_cache.insert(pda, account.into());
                        Ok(Some((pda, delegation)))
                    } else {
                        log::warn!(
                            "Account exists but couldn't deserialize data for PDA: {}. Data: {:?}", 
                            pda,
                            account.data()
                        );
                        Ok(None)
                    }
                } else {
                    log::warn!("Account data too short for PDA: {}", pda);
                    Ok(None)
                }
            }
            Err(e) => {
                log::info!("No account found for PDA: {} (Error: {})", pda, e);
                self.pda_cache.remove(&pda);
                Ok(None)
            }
        }
    }

    pub fn create_delegation_transaction(
        &mut self,
        user: &Pubkey,
        amount: u64,
    ) -> Result<Transaction> {
        // First check PDA existence
        let has_existing = self.get_or_fetch_pda(user)?.is_some();
        log::info!("create:::    signers are here: {:?}", self.signers);
        // Then get signer after PDA check
        let signer = self.signers.get(user)
            .ok_or_else(|| anyhow!("No delegation signer found for {}", user))?;

        let instruction = if has_existing {
            create_topup_instruction(user, amount)
        } else {
            create_delegation_instruction(user, amount)
        };

        let recent_blockhash = self.rpc_client.get_latest_blockhash()?;
        let message = Message::new_with_blockhash(
            &[instruction],
            Some(&signer.pubkey()),
            &recent_blockhash
        );
        
        let mut tx = Transaction::new_unsigned(message);
        tx.try_sign(&[signer], recent_blockhash)?;
        
        Ok(tx)
    }

    pub fn update_pda_state(&mut self, pda: Pubkey, account: AccountSharedData) {
        self.pda_cache.insert(pda, account);
    }

    pub fn create_withdrawal_transaction(&mut self, pda: &Pubkey, owner: &Pubkey, amount: u64) -> Result<Transaction> {
        let instruction = create_withdrawal_instruction(pda, owner, amount);
        log::info!("signers are here: {:?}", self.signers);
        // Get the signer for the owner
        let signer = self.signers.get(owner)
            .ok_or_else(|| anyhow!("No delegation signer found for {}", owner))?;
        
        let recent_blockhash = self.rpc_client.get_latest_blockhash()?;
        let message = Message::new(&[instruction], Some(&signer.pubkey()));
        
        let mut tx = Transaction::new_unsigned(message);
        tx.sign(&[signer], recent_blockhash);
        
        Ok(tx)
    }
    pub fn get_keypair(&self, user: &Pubkey) -> Option<&Keypair> {
        self.signers.get(user)
    }
} 
