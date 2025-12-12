// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::{Result, Context};
use clap::{Parser, Subcommand};
use cell_model::protocol::{MitosisRequest, MitosisResponse, TestEvent};
use tokio::net::UnixStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use colored::*;
use cell_model::rkyv::Deserialize;

#[derive(Parser)]
#[command(name = "cell")]
#[command(about = "Cell Substrate Control Interface", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Spawn a cell
    Spawn {
        name: String,
    },
    /// Run tests for a cell
    Test {
        target: String,
        #[arg(short, long)]
        filter: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Spawn { name } => spawn_cell(name).await,
        Commands::Test { target, filter } => run_test(target, filter).await,
    }
}

async fn connect_daemon() -> Result<UnixStream> {
    let home = dirs::home_dir().expect("No HOME");
    let socket_path = if let Ok(p) = std::env::var("CELL_SOCKET_DIR") {
        std::path::PathBuf::from(p).join("mitosis.sock")
    } else {
        home.join(".cell/runtime/system/mitosis.sock")
    };

    UnixStream::connect(&socket_path).await
        .context(format!("Could not connect to Hypervisor at {:?}. Is the system running?", socket_path))
}

async fn spawn_cell(name: String) -> Result<()> {
    let mut stream = connect_daemon().await?;
    let req = MitosisRequest::Spawn { cell_name: name.clone(), config: None };
    
    send_request(&mut stream, &req).await?;
    
    let resp: MitosisResponse = recv_response(&mut stream).await?;
    match resp {
        MitosisResponse::Ok { socket_path } => println!("{} Spawned {} at {}", "✔".green(), name, socket_path),
        MitosisResponse::Denied { reason } => println!("{} Spawn failed: {}", "✘".red(), reason),
    }
    Ok(())
}

async fn run_test(target: String, filter: Option<String>) -> Result<()> {
    let mut stream = connect_daemon().await?;
    let req = MitosisRequest::Test { target_cell: target.clone(), filter };
    
    send_request(&mut stream, &req).await?;
    
    println!("{} signaling test run for '{}'...", "Cell".blue().bold(), target);

    loop {
        // Read stream of TestEvents
        let mut len_buf = [0u8; 4];
        if stream.read_exact(&mut len_buf).await.is_err() {
            break; // EOF
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        stream.read_exact(&mut buf).await?;

        let event: TestEvent = cell_model::rkyv::check_archived_root::<TestEvent>(&buf)
            .expect("Invalid event data")
            .deserialize(&mut cell_model::rkyv::Infallible)
            .unwrap();

        match event {
            TestEvent::Log(msg) => println!("  {}", msg.dimmed()),
            TestEvent::CaseStarted(name) => print!("{} {} ... ", "RUN".yellow(), name),
            TestEvent::CaseFinished { name, success, duration_ms } => {
                if success {
                    println!("{} ({}ms)", "OK".green(), duration_ms);
                } else {
                    println!("{} ({}ms)", "FAIL".red(), duration_ms);
                }
            }
            TestEvent::SuiteFinished { total, passed, failed } => {
                println!("\nTest Result: {} passed, {} failed out of {}", passed, failed, total);
                if failed > 0 {
                    std::process::exit(1);
                }
                break;
            }
            TestEvent::Error(e) => {
                println!("{} {}", "ERROR:".red().bold(), e);
                std::process::exit(1);
            }
        }
    }
    Ok(())
}

async fn send_request<T: cell_model::rkyv::Serialize<cell_model::rkyv::ser::serializers::AllocSerializer<256>>>(stream: &mut UnixStream, req: &T) -> Result<()> {
    let bytes = cell_model::rkyv::to_bytes::<_, 256>(req)?.into_vec();
    stream.write_all(&(bytes.len() as u32).to_le_bytes()).await?;
    stream.write_all(&bytes).await?;
    Ok(())
}

async fn recv_response<T: cell_model::rkyv::Archive>(stream: &mut UnixStream) -> Result<T> 
where T::Archived: cell_model::rkyv::Deserialize<T, cell_model::rkyv::Infallible> + for<'a> cell_model::rkyv::CheckBytes<cell_model::rkyv::validation::validators::DefaultValidator<'a>>
{
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    
    let archived = cell_model::rkyv::check_archived_root::<T>(&buf)
        .map_err(|e| anyhow::anyhow!("Protocol error: {:?}", e))?;
    Ok(archived.deserialize(&mut cell_model::rkyv::Infallible).unwrap())
}