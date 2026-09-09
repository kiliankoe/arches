mod api;
mod config;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "arches",
    version,
    about = "Bridge between Arc Timeline Recorder backups and other tools."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the HTTP API and periodic ingest.
    Serve,
    /// Ingest the Arc backup into SQLite.
    Ingest,
    /// Show the last ingest run and row counts.
    Status,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();
    match cli.command {
        Command::Serve => serve().await,
        // Both need the schema and upsert logic that phase 2 adds.
        Command::Ingest | Command::Status => {
            eprintln!("not implemented yet (phase 2)");
            std::process::exit(2);
        }
    }
}

async fn serve() -> anyhow::Result<()> {
    let config = config::Config::from_env()?;
    // Everything arches writes goes here; created eagerly so phase 2's ingest can assume it exists.
    std::fs::create_dir_all(&config.data_dir)?;
    tracing::info!(
        arc_dir = %config.arc_dir.display(),
        data_dir = %config.data_dir.display(),
        ingest_interval = ?config.ingest_interval,
        "starting arches"
    );

    let state = api::AppState::new(&config.map_style);
    let app = api::router(state);

    let listener = tokio::net::TcpListener::bind(&config.bind).await?;
    tracing::info!("listening on http://{}", listener.local_addr()?);
    axum::serve(listener, app).await?;
    Ok(())
}
