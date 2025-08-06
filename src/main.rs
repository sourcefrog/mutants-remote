//! Launch cargo-mutants into AWS Batch jobs.

use std::path::{Path, PathBuf};
use std::{env::temp_dir, fs::File};

use clap::{Parser, Subcommand};
use thiserror::Error;
use tokio::process::Command;
use tracing::level_filters::LevelFilter;
use tracing::{error, info};
use tracing_subscriber::{Layer, filter::filter_fn, fmt, layer::SubscriberExt};

mod cloud;
use crate::cloud::{Cloud, aws::AwsCloud};

static TOOL_NAME: &str = "mutants-remote";
static SOURCE_TARBALL_NAME: &str = "source.tar.zstd";

#[derive(Parser)]
#[command(name = "mutants-remote")]
#[command(about = "Launch cargo-mutants into AWS Batch jobs")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a mutants test suite
    Run {
        /// Source directory to run mutants on
        #[arg(long, short = 'd')]
        source: PathBuf,

        /// Total number of shards
        #[arg(long, default_value = "100")]
        shards: u32,
    },
}

// TODO: Also try `thistermination` to give specific error codes...
#[derive(Error, Debug)]
pub enum Error {
    #[error("Cloud error: {0}")]
    Cloud(#[from] cloud::CloudError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Tar failed: {0}")]
    Tar(String),
    // #[allow(dead_code)]
    // #[error("Job failed with status: {0}")]
    // JobFailed(String),

    // #[allow(dead_code)]
    // #[error("Job timed out")]
    // JobTimeout,

    // #[allow(dead_code)]
    // #[error("Invalid configuration: {0}")]
    // Config(String),
}

/// A general description of the suite to run, including the config.
#[derive(Debug, Clone)]
pub struct Suite {
    pub suite_id: String,
    pub config: Config,
}

/// User-provided configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub aws_s3_bucket: String,
    pub aws_batch_job_queue: String,
    pub aws_batch_job_definition: String,
    pub aws_log_group_name: String,
}

#[tokio::main]
#[allow(clippy::result_large_err)]
async fn main() -> Result<(), Error> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run { source, shards } => run_command(source, shards).await,
    }
}

async fn run_command(source_dir: PathBuf, shards: u32) -> Result<(), Error> {
    let suite_id = suite_id();
    setup_tracing(&suite_id);

    // Create job configuration
    let config = Config {
        aws_s3_bucket: "mutants-tmp-0733-uswest2".to_string(),
        aws_batch_job_queue: "mutants0-amd64".to_string(),
        aws_batch_job_definition: "mutants0-amd64".to_string(),
        aws_log_group_name: "/aws/batch/job".to_string(),
    };
    let suite = Suite {
        suite_id: suite_id.clone(),
        config,
    };

    // Create AWS cloud provider
    let cloud = match AwsCloud::new(suite.clone()).await {
        Ok(cloud) => cloud,
        Err(err) => {
            error!("Failed to initialize AWS cloud: {err}");
            return Err(Error::Cloud(err));
        }
    };

    let source_tarball = tar_source(&source_dir).await?;
    match cloud.upload_source_tarball(&source_tarball).await {
        Ok(()) => {}
        Err(err) => {
            error!("Failed to upload source tarball: {err}");
            return Err(Error::Cloud(err));
        }
    }

    // TODO: Maybe run the baseline once and then copy it, with <https://github.com/sourcefrog/cargo-mutants/issues/541>

    // Submit job
    let shard_k = 0;
    let script = format!("cargo mutants --shard {shard_k}/{shards} -vV || true");
    let job_name = format!(
        "{TOOL_NAME}-{suite_id}-shard-{shard_k}",
        suite_id = suite.suite_id
    );
    // TODO: Maybe pass in the shard_k and shard_n to be used as tags?
    info!(?suite_id, ?job_name, "Submitting job");
    let job_id = match cloud.submit_job(script, job_name).await {
        Ok(id) => id,
        Err(err) => {
            error!("Failed to submit job: {err}");
            return Err(Error::Cloud(err));
        }
    };

    // Monitor job
    if let Err(err) = cloud.monitor_job(&job_id).await {
        error!("Failed to monitor job: {err}");
        return Err(Error::Cloud(err));
    }

    // Fetch output
    match cloud.fetch_output(&job_id).await {
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

/// Tar up the source directory and return the temporary path to the tarball.
async fn tar_source(source: &Path) -> Result<PathBuf, Error> {
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

fn setup_tracing(suite_id: &str) {
    let log_path = temp_dir().join(format!("{TOOL_NAME}-{suite_id}.log"));
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

fn suite_id() -> String {
    let now = chrono::Local::now();
    let time_str = now.format("%Y%m%d%H%M%S").to_string();
    format!(
        "{time}-{random:04x}",
        time = time_str,
        random = fastrand::u16(..)
    )
}
