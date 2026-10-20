//! The `flamed` binary.
//!
//! This file gates devnet code on `feature = "devnet"` alone, unlike the
//! library, which gates on `any(test, feature = "devnet")`. It has to:
//! `cargo test` builds this target's own test harness with `cfg(test)` on,
//! but links the library built *without* it, so an `any(test, ..)` arm here
//! would name items that library does not have. For the same reason every
//! devnet `use` lives inside a gated body.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use clap::{Parser, Subcommand};
use flamed::config::{GenesisFile, NodeConfig};
use flamed::node::{Node, SharedNode};

/// The Flame node.
#[derive(Parser)]
#[command(name = "flamed", about = "The Flame node", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Derives genesis.json from a network definition. Devnet builds only.
    #[cfg(feature = "devnet")]
    Genesis {
        /// The network definition to read.
        #[arg(long)]
        chainparams: PathBuf,
        /// Where to write the genesis.
        #[arg(long, default_value = "./genesis.json")]
        out: PathBuf,
    },
    /// Prints the first receiving address of the wallet whose BIP-39
    /// mnemonic is read from stdin, with no passphrase: the address a
    /// wallet restored from that mnemonic shows first. Devnet builds only.
    #[cfg(feature = "devnet")]
    Address {
        /// The network definition whose `network` the address is for.
        #[arg(long)]
        chainparams: PathBuf,
    },
    /// Opens a chain and serves it.
    Run {
        /// This node's policy file.
        #[arg(long, default_value = "./flamed.toml")]
        config: PathBuf,
    },
}

fn main() {
    // `Display`, not the `Debug` a `Result`-returning main would print: an
    // error here is for an operator to read, and several of them say what
    // to do about it.
    if let Err(error) = dispatch() {
        eprintln!("flamed: {error}");
        std::process::exit(1);
    }
}

fn dispatch() -> Result<(), Box<dyn std::error::Error>> {
    match Cli::parse().command {
        #[cfg(feature = "devnet")]
        Command::Genesis { chainparams, out } => {
            let chainparams = flamed::config::ChainParamsFile::load(&chainparams)?;
            let genesis = flamed::genesis::write(&chainparams, &out)?;
            println!(
                "genesis {} with {} allocation(s) -> {}",
                genesis.genesis_hash,
                genesis.contracts.len(),
                out.display()
            );
            Ok(())
        }
        #[cfg(feature = "devnet")]
        Command::Address { chainparams } => {
            println!("{}", first_address(&chainparams)?);
            Ok(())
        }
        // Built by hand rather than with `#[tokio::main]`, because
        // `flamed genesis` is synchronous and must not need a runtime.
        Command::Run { config } => tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(run(config)),
    }
}

/// `m/35263'/network'/0'/0/0` of the mnemonic on stdin, in bech32f. The
/// phrase is a secret: it is never echoed, not even in an error, since a
/// parse error can name the word it stopped at.
#[cfg(feature = "devnet")]
fn first_address(chainparams: &std::path::Path) -> Result<String, Box<dyn std::error::Error>> {
    use std::io::Read;

    use flamekd::{util, Mnemonic, Network};
    use flamepayments::SpendAccount;
    use zeroize::Zeroizing;

    let network: Network = flamed::config::ChainParamsFile::load(chainparams)?
        .network
        .into();
    let mut phrase = Zeroizing::new(String::new());
    std::io::stdin().read_to_string(&mut phrase)?;
    let mnemonic =
        Mnemonic::parse(phrase.trim()).map_err(|_| "stdin is not a valid BIP-39 mnemonic")?;
    let seed = Zeroizing::new(mnemonic.to_seed(""));
    let address = SpendAccount::from_seed(&seed, network, 0)?
        .address_at(util::RECEIVING, 0)?
        .to_bech32(network);
    Ok(address)
}

async fn run(config: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let cfg = NodeConfig::load(&config)?;
    let genesis = GenesisFile::load(&cfg.genesis_path())?;
    let node: SharedNode = Arc::new(Mutex::new(Node::open(&genesis, &cfg)?));
    {
        let opened = node.lock().expect("a freshly built node is not poisoned");
        println!(
            "flamed: height {} with {} unspent contract(s), {} block(s) archived",
            opened.tip().height,
            opened.utxo_count(),
            opened.block_count()
        );
    }

    let (addr, handle) = flamed::rpc::serve(Arc::clone(&node), cfg.rpc_bind).await?;
    println!("flamed: serving JSON-RPC on http://{addr}");

    #[cfg(feature = "devnet")]
    {
        tokio::spawn(flamed::minter::Minter::new(Arc::clone(&node), cfg.block_interval()).run());
        println!("flamed: minting every {}s", cfg.block_interval_secs);
    }

    // Never resolves unless something stops the server, and holds the last
    // handle so the server is not dropped out from under itself.
    handle.stopped().await;
    Ok(())
}
