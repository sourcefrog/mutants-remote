// Copyright 2025 Martin Pool

//! Launch cargo-mutants into AWS Batch jobs.
//!
//! # Concepts
//!
//! One "run" corresponds to testing all the selected mutants in one tree, and is launched
//! by `mutants-remote run`.
//!
//! In future, one run may be split into multiple jobs, each of which runs a shard of mutants.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;
use std::{env::temp_dir, fs::File};

use clap::{Parser, Subcommand};
use time::{OffsetDateTime, macros::format_description};
use tokio::process::Command;
use tokio::time::sleep;
use tracing::level_filters::LevelFilter;
use tracing::{error, info};
use tracing_subscriber::{Layer, filter::filter_fn, fmt, layer::SubscriberExt};

mod cloud;
use crate::cloud::{Cloud, CloudJobId, aws::AwsCloud};
mod config;
use crate::config::Config;
mod error;
use crate::error::Error;

static TOOL_NAME: &str = "mutants-remote";
static SOURCE_TARBALL_NAME: &str = "source.tar.zstd";

#[derive(Parser)]
#[command(name = "mutants-remote")]
#[command(about = "Launch cargo-mutants into AWS Batch jobs")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(long, global = true)]
    config: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Commands {
    /// Remotely test cargo-mutants on a source directory
    #[command(alias = "r")]
    Run {
        /// Source directory to run mutants on
        #[arg(long, short = 'd')]
        source: PathBuf,

        /// Total number of shards
        #[arg(long, default_value = "10")]
        shards: u32,
    },
}

type Result<T> = std::result::Result<T, Error>;

/// Describes the status of a job.
#[derive(Debug, Copy, derive_more::Display, Clone, PartialEq, Eq, Hash)]
pub enum JobStatus {
    Submitted,
    Pending,
    Starting,
    Runnable,
    Running,
    Completed,
    Stopping,
    Failed,
    Unknown,
}

/// Identifier assigned by us to a run.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RunId(String);

/// Name assigned by us to a job within a run, including the run id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct JobName {
    run_id: RunId,
    // TODO: Maybe later an enum allowing for separate baseline and mutants jobs.
    shard_k: u32,
}

#[tokio::main]
#[allow(clippy::result_large_err)]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    let result = match &cli.command {
        Commands::Run { source, shards } => run_command(&cli, source, *shards).await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            error!("{err}");
            ExitCode::FAILURE
        }
    }
}

async fn run_command(cli: &Cli, source_dir: &Path, shards: u32) -> Result<()> {
    let run_id = RunId::new();
    setup_tracing(&run_id);

    let config = if let Some(config_path) = &cli.config {
        Config::from_file(config_path)?
    } else {
        Config::default()
    };

    // Create AWS cloud provider
    let cloud = match AwsCloud::new(config.clone()).await {
        Ok(cloud) => cloud,
        Err(err) => {
            error!("Failed to initialize AWS cloud: {err}");
            return Err(Error::Cloud(err));
        }
    };

    let source_tarball_path = tar_source(source_dir).await?;
    match cloud
        .upload_source_tarball(&run_id, &source_tarball_path)
        .await
    {
        Ok(()) => {}
        Err(err) => {
            error!("Failed to upload source tarball: {err}");
            return Err(Error::Cloud(err));
        }
    }

    // TODO: Maybe run the baseline once and then copy it, with <https://github.com/sourcefrog/cargo-mutants/issues/541>

    // Submit job
    let shard_k = 0;
    let script = format!("cargo mutants --in-place --shard {shard_k}/{shards} -vV || true");
    let job_name = JobName {
        run_id: run_id.clone(),
        shard_k,
    };
    info!(?job_name, "Submitting job");
    let job_id = match cloud.submit_job(&job_name, script).await {
        Ok(id) => id,
        Err(err) => {
            error!("Failed to submit job: {err}");
            return Err(Error::Cloud(err));
        }
    };

    // Monitor job
    let _final_status = match monitor_job(&cloud, &job_id).await {
        Ok(status) => status,
        Err(err) => {
            error!("Failed to monitor job: {err}");
            return Err(err);
        }
    };

    // Fetch output
    match cloud.fetch_output(&job_name).await {
        Ok(output_path) => {
            info!(
                "Job completed successfully. Output available at: {}",
                output_path.display()
            );
        }
        Err(err) => {
            error!("Failed to fetch output: {err}");
            return Err(Error::Cloud(err));
        }
    }

    Ok(())
}

async fn monitor_job(cloud: &dyn Cloud, job_id: &CloudJobId) -> Result<JobStatus> {
    let mut last_status: Option<JobStatus> = None;
    let mut log_tail = None;
    let mut _logs_ended = false;
    loop {
        let job_description = cloud.describe_job(job_id).await?;
        let status = job_description.status;
        if last_status != Some(status) {
            info!(?job_id, "Job status changed to {status}");
            last_status = Some(job_description.status);
        }
        match job_description.status {
            JobStatus::Completed | JobStatus::Failed => {
                // TODO: Maybe wait just a little longer for the last logs? But, we don't seem to reliably detect the end of the logs.
                return Ok(job_description.status);
            }
            JobStatus::Running => {
                if log_tail.is_none() && job_description.log_stream_name.is_some() {
                    log_tail = Some(cloud.tail_log(&job_description).await?);
                }
            }
            _ => {}
        }
        if let Some(log_tail) = &mut log_tail {
            match log_tail.more_log_events().await {
                Ok(Some(events)) => {
                    for message in events {
                        for line in message.split(['\n', '\r']) {
                            // some messages have just \r
                            println!("    {line}");
                        }
                    }
                }
                Ok(None) => {
                    info!("End of log stream");
                    _logs_ended = true;
                }
                Err(e) => {
                    error!("Error fetching log events: {}", e);
                    // logs_ended here? unclear.
                }
            }
        }
        sleep(Duration::from_secs(1)).await;
    }
}

/// Tar up the source directory and return the temporary path to the tarball.
async fn tar_source(source: &Path) -> Result<PathBuf> {
    let temp_dir = temp_dir();
    let tarball_path = temp_dir.join(SOURCE_TARBALL_NAME);
    let mut child = Command::new("tar")
        .arg("--zstd")
        .arg("-cf")
        .arg(&tarball_path)
        .arg("-C")
        .arg(source)
        .arg("--exclude")
        .arg("target")
        .arg(".")
        .spawn()
        .map_err(Error::Io)?;
    let exit_status = child.wait().await.map_err(Error::Io)?;
    if !exit_status.success() {
        return Err(Error::Tar(format!("tar failed: {exit_status}")));
    }
    Ok(tarball_path)
}

fn setup_tracing(run_id: &RunId) {
    let log_path = temp_dir().join(format!("{TOOL_NAME}-{}.log", run_id.0));
    let log_file = File::create(&log_path).unwrap();
    let file_layer = fmt::Layer::new()
        .with_ansi(false)
        .with_file(true)
        .with_line_number(true)
        .with_target(true)
        .with_level(true)
        .with_writer(log_file)
        .with_filter(filter_fn(|metadata| {
            metadata.target().starts_with("mutants_remote")
        }))
        .with_filter(LevelFilter::DEBUG);
    let stderr_layer = fmt::Layer::new()
        .with_target(true)
        .with_level(true)
        .with_writer(std::io::stderr)
        .with_filter(LevelFilter::INFO); // EnvFilter::from_default_env());
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry()
            .with(stderr_layer)
            .with(file_layer),
    )
    .unwrap();
    info!("Tracing initialized to file {}", log_path.display());
}

impl RunId {
    /// Generate a unique ID for a run.
    fn new() -> RunId {
        let now = OffsetDateTime::now_utc();
        let time_str = now
            .format(format_description!(
                "[year][month][day][hour][minute][second]"
            ))
            .unwrap();
        RunId(format!(
            "{time}-{random:04x}",
            time = time_str,
            random = fastrand::u16(..)
        ))
    }
}

impl std::fmt::Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::fmt::Display for JobName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-shard-{}", self.run_id, self.shard_k)
    }
}
