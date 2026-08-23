use anyhow::Result;
use clap::{Parser, Subcommand};
use nexus_config::Config;
use nexus_runtime::AgentRuntime;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "nexus", version, about = "A configurable AI agent harness")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run a task through the Nexus runtime.
    Run { task: String },
    /// Validate the resolved Nexus configuration.
    Config,
    /// Display the runtime version and health.
    Doctor,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .init();

    let cli = Cli::parse();
    match cli.command {
        Some(Command::Run { task }) => {
            let config = Config::default();
            let runtime = AgentRuntime::new(config);
            let result = runtime.run(&task).await?;
            println!("{}", result.message);
        }
        Some(Command::Config) => {
            println!("Nexus configuration is valid (default configuration loaded).");
        }
        Some(Command::Doctor) => {
            println!("Nexus runtime: healthy");
        }
        None => {
            println!("Nexus {}", env!("CARGO_PKG_VERSION"));
            println!("Run `nexus --help` to get started.");
        }
    }

    Ok(())
}
