//! One-shot binary to create a durable nonce account.
//! Reads BOT_PRIVATE_KEY from env, creates nonce, prints address.
//! Usage: cargo run --bin create_nonce --release

use solana_client::rpc_client::RpcClient;
use solana_sdk::instruction::Instruction;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::system_instruction;

fn main() -> anyhow::Result<()> {
    let pk_b58 =
        std::env::var("BOT_PRIVATE_KEY").expect("BOT_PRIVATE_KEY env var not set");
    let wallet = Keypair::from_base58_string(&pk_b58);

    let rpc_url = "https://api.mainnet-beta.solana.com";
    let rpc = RpcClient::new_with_timeout(rpc_url.to_string(), std::time::Duration::from_secs(30));

    let balance = rpc.get_balance(&wallet.pubkey())?;
    println!(
        "Wallet: {}, balance: {:.6} SOL",
        wallet.pubkey(),
        balance as f64 / 1e9,
    );

    let nonce = Keypair::new();
    let rent = rpc.get_minimum_balance_for_rent_exemption(80)?;
    println!("Nonce account candidate: {}", nonce.pubkey());
    println!("Rent-exempt cost: {:.6} SOL", rent as f64 / 1e9);

    let mut ixs: Vec<Instruction> = system_instruction::create_nonce_account(
        &wallet.pubkey(),
        &nonce.pubkey(),
        &wallet.pubkey(),
        rent,
    );

    let blockhash = rpc.get_latest_blockhash()?;
    let tx = solana_sdk::transaction::Transaction::new_signed_with_payer(
        &ixs,
        Some(&wallet.pubkey()),
        &[&wallet, &nonce],
        blockhash,
    );

    println!("Sending nonce creation TX...");
    let sig = rpc.send_and_confirm_transaction(&tx)?;

    println!();
    println!("Nonce account created: {}", nonce.pubkey());
    println!("TX: {}", sig);
    println!();
    println!("Add this to config.toml:");
    println!("nonce_account = \"{}\"", nonce.pubkey());

    Ok(())
}
