use sha2::{Sha256, Digest};
use solana_sdk::{
    pubkey::Pubkey,
    instruction::{AccountMeta, Instruction},
    system_program,
};
use borsh::{BorshSerialize, BorshDeserialize};


#[derive(BorshSerialize, BorshDeserialize)]
pub struct DelegatedAccount {
    pub owner: Pubkey,
    pub delegated_amount: u64,
    pub last_deposit_time: i64,
    pub bump: u8,
}


#[derive(BorshSerialize)]
pub struct InitializeDelegateArgs {
    pub amount: u64,
}

pub fn get_delegation_program_id() -> Pubkey {
    "5MSF4TiUfD7dVm7P1ahPYJfEBLCUQn7hEPYXYHocVwzh"
        .parse()
        .unwrap()
}

pub fn find_delegation_pda(owner: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[b"delegate", owner.as_ref()],
        &get_delegation_program_id()
    )
}

pub fn create_delegation_instruction(owner: &Pubkey, amount: u64) -> Instruction {
    let (pda, _) = find_delegation_pda(owner);
    
    let discriminator = {
        let mut hasher = Sha256::new();
        hasher.update(b"global:initialize_delegate");
        let result = hasher.finalize();
        let mut disc = [0u8; 8];
        disc.copy_from_slice(&result[..8]);
        disc
    };
    
    let mut ix_data = discriminator.to_vec();
    ix_data.extend(InitializeDelegateArgs { amount }.try_to_vec().unwrap());

    Instruction {
        program_id: get_delegation_program_id(),
        accounts: vec![
            AccountMeta::new(*owner, true),              // Owner must be signer
            AccountMeta::new(pda, false),                // PDA account
            AccountMeta::new_readonly(system_program::id(), false), // System program
        ],
        data: ix_data,
    }
}

pub fn create_topup_instruction(owner: &Pubkey, amount: u64) -> Instruction {
    let (pda, _) = find_delegation_pda(owner);
    
    // Calculate Anchor discriminator for "top_up"
    let discriminator = {
        let mut hasher = Sha256::new();
        hasher.update(b"global:top_up");
        let result = hasher.finalize();
        let mut disc = [0u8; 8];
        disc.copy_from_slice(&result[..8]);
        disc
    };
    
    let mut ix_data = discriminator.to_vec();
    ix_data.extend(amount.to_le_bytes());

    Instruction {
        program_id: get_delegation_program_id(),
        accounts: vec![
            AccountMeta::new(*owner, true),              // Owner must be signer
            AccountMeta::new(pda, false),                // PDA to be topped up
            AccountMeta::new_readonly(system_program::id(), false), // System program
        ],
        data: ix_data,
    }
}

pub fn create_withdrawal_instruction(pda: &Pubkey, owner: &Pubkey, amount: u64) -> Instruction {
    // Calculate Anchor discriminator for "withdraw"
    let discriminator = {
        let mut hasher = Sha256::new();
        hasher.update(b"global:withdraw");
        let result = hasher.finalize();
        let mut disc = [0u8; 8];
        disc.copy_from_slice(&result[..8]);
        disc
    };
    
    let mut ix_data = discriminator.to_vec();
    ix_data.extend(amount.to_le_bytes());

    Instruction {
        program_id: get_delegation_program_id(),
        accounts: vec![
            AccountMeta::new(*owner, true),         // Owner must be a signer
            AccountMeta::new(*pda, false),          // PDA account
            AccountMeta::new_readonly(system_program::id(), false), // System program
        ],
        data: ix_data,
    }
} 