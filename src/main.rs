mod api;
mod arc;
mod config;
mod db;
mod ingest;
mod status;

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
    Status {
        /// Print the raw status struct as JSON.
        #[arg(long)]
        json: bool,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();
    match cli.command {
        Command::Serve => serve().await,
        Command::Ingest => ingest(),
        Command::Status { json } => show_status(json),
    }
}

fn ingest() -> anyhow::Result<()> {
    let config = config::Config::from_env()?;
    let mut conn = db::open(&config.db_path())?;
    let summary = ingest::run(&mut conn, &config)?;

    println!(
        "run {}: {} of {} files ingested in {:.1}s ({} places, {} items, {} samples upserted)",
        summary.id,
        summary.files_ingested,
        summary.files_seen,
        summary.elapsed_ms as f64 / 1000.0,
        summary.places_upserted,
        summary.items_upserted,
        summary.samples_upserted,
    );
    if let Some(error) = &summary.error {
        eprintln!("{error}");
        // Partial ingest is still a failure for whatever called us.
        std::process::exit(1);
    }
    Ok(())
}

fn show_status(json: bool) -> anyhow::Result<()> {
    let config = config::Config::from_env()?;
    let conn = db::open(&config.db_path())?;
    let status = status::status(&conn)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&status)?);
        return Ok(());
    }

    println!(
        "places {}, items {}, samples {}, files {}",
        status.counts.places, status.counts.items, status.counts.samples, status.counts.files
    );
    println!(
        "items {} .. {}",
        format_millis(status.first_item_start),
        format_millis(status.last_item_start)
    );
    println!("last ingest at {}", format_millis(status.last_ingested_at));
    match &status.last_run {
        None => println!("no ingest run yet"),
        Some(run) => {
            println!(
                "run {} started {}, finished {}, device {}",
                run.id,
                format_millis(Some(run.started_at)),
                format_millis(run.finished_at),
                run.device_id.as_deref().unwrap_or("-")
            );
            println!(
                "  {} of {} files, +{} places, +{} items, +{} samples",
                run.files_ingested,
                run.files_seen,
                run.places_upserted,
                run.items_upserted,
                run.samples_upserted
            );
            if let Some(error) = &run.error {
                println!("  error: {error}");
            }
        }
    }
    Ok(())
}

fn format_millis(millis: Option<i64>) -> String {
    millis
        .and_then(|millis| jiff::Timestamp::from_millisecond(millis).ok())
        .map(|timestamp| timestamp.to_string())
        .unwrap_or_else(|| "-".to_string())
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
