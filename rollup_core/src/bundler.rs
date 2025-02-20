use std::collections::HashMap;

use solana_sdk::{instruction::{CompiledInstruction, Instruction}, pubkey::Pubkey, system_instruction::{self, SystemInstruction}, system_program, transaction::Transaction};
use bincode::deserialize;

pub fn get_transaction_instructions(tx: &Transaction) -> Vec<CompiledInstruction>{
    tx.message.instructions.clone()
}

pub fn is_transfer_ix(cix: &CompiledInstruction, account_keys: &[Pubkey]) -> bool {
    if cix.program_id_index as usize >= account_keys.len(){
        return false;
    }
    let program_id = account_keys[cix.program_id_index as usize];
    if program_id != system_program::ID{
        return false
    }
    
    matches!(
        deserialize::<SystemInstruction>(&cix.data),
        Ok(SystemInstruction::Transfer { .. })
    )
}

#[derive(PartialEq, Eq, Hash, Clone, Copy)]
struct TBundlerKey {
    keys: [Pubkey; 2],
}

pub struct TransferBundler {
    transfers: HashMap<TBundlerKey, i128>,
}

impl TransferBundler {
    pub fn new() -> Self {
        Self {
            transfers: HashMap::new()
        }
    }

    pub fn parse_compiled_instruction(ix: &CompiledInstruction, account_keys: &[Pubkey]) -> Option<(Pubkey, Pubkey, i128)>{
        //Ensure the instruction is from System Program (where transfer is from)
        if ix.program_id_index as usize >= account_keys.len()  || account_keys[ix.program_id_index as usize] != system_program::ID{
            return None;
        }
        //Ensure we have at least 2 accounts for transfer and enough data for SOL amount
        if ix.accounts.len() < 2 || ix.data.len() < 8 {
            return None;
        }

        //Get accounts involved in transfer and amount to be transferred
        let from = account_keys[ix.accounts[0] as usize];
        let to = account_keys[ix.accounts[1] as usize];

        log::info!("FROM: {:?}", from.to_string());
        log::info!("TO: {:?}", to.to_string());
        log::info!("IX DATA: {:?}", ix.data);

        let amount = u64::from_le_bytes(ix.data[4..12].try_into().ok()?);
        Some((from, to, amount as i128))
    }

    pub fn parse_instruction(ix: &Instruction) -> Option<(Pubkey, Pubkey, i128)>{
        //Enusre ix is owned by system program
        if ix.program_id != system_program::ID{
            return None;
        }

        //Ensure we have enough accounts
        if ix.accounts.len() < 2{
            return None;
        }
        let from = ix.accounts[0].pubkey;
        let to = ix.accounts[1].pubkey;
        let amount = u64::from_le_bytes(ix.data[4..].try_into().ok()?);
        
        log::info!("FROM: {:?}", from.to_string());
        log::info!("TO: {:?}", to.to_string());
        log::info!("AMOUNT: {amount}");
        log::info!("IX DATA: {:?}", ix.data);

        
        Some((from, to, amount as i128))
    }

    //Parses transactions and add transfer ixs to TransferBundler
    pub fn bundle(&mut self, transaction: Transaction){
        let ixs = get_transaction_instructions(&transaction);
        let account_keys: &[Pubkey] = &transaction.message.account_keys;
        for ix in ixs {
            if is_transfer_ix(&ix, account_keys){
                let (from, to, amount) = Self::parse_compiled_instruction(&ix, account_keys).unwrap();
                let mut keys = [from, to];
                keys.sort();
                
                *self.transfers.entry(TBundlerKey {keys}).or_default() += if from == keys[0] {amount} else {-amount};
            }
        }
    }

    pub fn generate_final(self) -> Vec<Instruction> {
        self.transfers.into_iter().filter_map(|(map_key, val)| {
            if val < 0 {
                Some(system_instruction::transfer(&map_key.keys[1], &map_key.keys[0], val.unsigned_abs() as u64))
            } else if val > 0 {
                Some(system_instruction::transfer(&map_key.keys[0], &map_key.keys[1], val as u64))
            } else {
                None
            }
        }).collect()
    }
}