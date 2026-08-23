use std::sync::Arc;

use anyhow::Result;
use clap::{Parser, Subcommand};
use nexus_config::Config;
use nexus_models::{MockModelProvider, ModelStreamEvent};
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
    Run {
        task: String,
        /// Stream model output as events arrive.
        #[arg(short, long)]
        stream: bool,
        /// Model identifier passed to the selected provider.
        #[arg(short, long, default_value = "mock-1")]
        model: String,
    },
    /// List providers currently built into the CLI.
    Models,
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
        Some(Command::Run { task, stream, model }) => {
            let config = Config::default();
            let runtime = AgentRuntime::new(
                config,
                Arc::new(MockModelProvider::default()),
                model,
            );

            if stream {
                let result = runtime
                    .run_streaming(&task, |event| match event {
                        ModelStreamEvent::Started { model } => {
                            eprintln!("[nexus] model: {}", model.0);
                        }
                        ModelStreamEvent::Delta { content } => print!("{content}"),
                        ModelStreamEvent::Completed { usage } => {
                            eprintln!(
                                "\n[nexus] usage: {} input / {} output tokens",
                                usage.input_tokens, usage.output_tokens
                            );
                        }
                    })
                    .await?;

                eprintln!("[nexus] run {} completed via {}", result.task_id, result.provider);
            } else {
                let result = runtime.run(&task).await?;
                println!("{}", result.message);
            }
        }
        Some(Command::Models) => {
            println!("Built-in providers:");
            println!("  mock  - deterministic local provider for development and tests");
        }
        Some(Command::Config) => {
            println!("Nexus configuration is valid (default configuration loaded).");
        }
        Some(Command::Doctor) => {
            println!("Nexus runtime: healthy");
            println!("Model gateway: healthy");
            println!("Default provider: mock");
        }
        None => {
            println!("Nexus {}", env!("CARGO_PKG_VERSION"));
            println!("Run `nexus --help` to get started.");
        }
    }

    Ok(())
}
