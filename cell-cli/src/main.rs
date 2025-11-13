use anyhow::Result;
use clap::{Parser, Subcommand};
use std::process::Command;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "cell")]
#[command(about = "Cell microservice orchestrator")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start a service in the background
    Start {
        name: String,
        binary: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    
    match cli.command {
        Commands::Start { name, binary } => {
            std::fs::create_dir_all("/tmp/cell/sockets")?;
            std::fs::create_dir_all("/tmp/cell/logs")?;
            
            let log_file = std::fs::File::create(format!("/tmp/cell/logs/{}.log", name))?;
            
            Command::new(binary)
                .env("CELL_SOCKET_PATH", format!("/tmp/cell/sockets/{}.sock", name))
                .stdout(log_file.try_clone()?)
                .stderr(log_file)
                .spawn()?;
            
            println!("✓ Started service '{}'", name);
            Ok(())
        }
    }
}
