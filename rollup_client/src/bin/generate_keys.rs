use solana_sdk::{
    signature::Keypair,
    signer::Signer,
};
use std::fs;
use std::path::Path;

fn main() {
    // Create keypairs
    let sender = Keypair::new();
    let receiver = Keypair::new();
    
    // Create keys directory if it doesn't exist
    let keys_dir = Path::new("keys");
    fs::create_dir_all(keys_dir).unwrap();
    
    // Save sender keypair
    fs::write(
        keys_dir.join("sender.json"),
        sender.to_bytes().to_vec()
    ).unwrap();
    
    // Save receiver keypair
    fs::write(
        keys_dir.join("receiver.json"),
        receiver.to_bytes().to_vec()
    ).unwrap();
    
    // Print public keys
    println!("Generated keypairs:");
    println!("Sender pubkey: {} (saved to keys/sender.json)", sender.pubkey());
    println!("Receiver pubkey: {} (saved to keys/receiver.json)", receiver.pubkey());
    println!("\nPlease airdrop SOL to these addresses using the Solana CLI or devnet faucet");
} 