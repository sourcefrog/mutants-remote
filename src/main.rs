// Copyright 2025 Martin Pool

//! Launch cargo-mutants into AWS Batch jobs.
//!
//! # Concepts
//!
//! One "run" corresponds to testing all the selected mutants in one tree, and is launched
//! by `mutants-remote run`.
//!
//! In future, one run may be split into multiple jobs, each of which runs a shard of mutants.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use schemars::schema_for;
use tempfile::TempDir;
use time::OffsetDateTime;
use tokio::process::Command;
use tokio::time::sleep;
use tracing::level_filters::LevelFilter;
use tracing::{debug, error, info};
use tracing_subscriber::{Layer, filter::filter_fn, fmt, layer::SubscriberExt};

mod cloud;
mod config;
mod error;
mod job;
mod run;
mod shorttime;
mod tags;

use crate::cloud::{Cloud, CloudJobId, open_cloud};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::job::JobStatus;
use crate::run::{KillTarget, RunId, RunMetadata};

static VERSION: &str = env!("CARGO_PKG_VERSION");
static TOOL_NAME: &str = "mutants-remote";
static SOURCE_TARBALL_NAME: &str = "source.tar.zstd";

#[derive(Parser)]
#[command(name = "mutants-remote")]
#[command(about = "Launch cargo-mutants into AWS Batch jobs")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Path to mutants-remote configuration file.
    ///
    /// If not provided, the default is ~/.config/mutants-remote.toml. If that does
    /// not exist, built-in defaults will be used.
    #[arg(long, global = true)]
    config: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Commands {
    /// Remotely test cargo-mutants on a source directory
    #[command(visible_alias = "r")]
    Run {
        /// Source directory to run mutants on
        #[arg(long, short = 'd')]
        source: PathBuf,

        /// Total number of shards
        #[arg(long, default_value = "1")]
        shards: u32,
    },

    /// Print the JSON schema for the configuration file.
    ConfigSchema {},

    /// List jobs
    #[command(visible_alias = "ls")]
    List {
        // TODO: Options for the queue, cutoff time, other filters?
        /// List all known fields.
        #[arg(long, short = 'v')]
        verbose: bool,

        /// Output in JSON format.
        #[arg(long, short = 'j')]
        json: bool,

        /// Display runs that started within a recent period.
        #[arg(long, short = 's', default_value = "1 day")]
        since: String,
    },

    /// Kill runs
    Kill {
        /// Run ID to kill
        #[arg(long, short = 'i', required_unless_present = "all")]
        run_id: Vec<RunId>,

        /// Kill all runs
        #[arg(long, conflicts_with = "run_id")]
        all: bool,
    },
}

#[tokio::main]
#[allow(clippy::result_large_err)]
async fn main() -> ExitCode {
    match inner_main().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            error!("{err}");
            ExitCode::FAILURE
        }
    }
}

async fn inner_main() -> Result<()> {
    let cli = Cli::parse();
    let run_id = RunId::from_clock();
    let tempdir = TempDir::with_prefix(format!("{TOOL_NAME}-{run_id}-"))?.keep();
    setup_tracing(&tempdir);

    let config = Config::new(&cli.config)?;
    debug!(?config);
    let cloud = open_cloud(&config).await?;

    match &cli.command {
        Commands::Run { shards, source } => {
            assert_eq!(*shards, 1, "Multiple shards are not supported yet");
            let run_metadata = RunMetadata::new(source);
            let source_tarball_path = tar_source(source, &tempdir).await?;
            cloud
                .upload_source_tarball(&run_id, &source_tarball_path)
                .await
                .inspect_err(|err| error!("Failed to upload source tarball: {err}"))?;

            // TODO: Maybe run the baseline once and then copy it, with <https://github.com/sourcefrog/cargo-mutants/issues/541>

            info!("Submitting job");
            let (job_name, cloud_job_id) = cloud
                .submit(&run_id, &run_metadata)
                .await
                .inspect_err(|err| error!("Failed to submit job: {err}"))?;

            // Monitor job
            let (_final_status, started_running) = monitor_job(cloud.as_ref(), &cloud_job_id)
                .await
                .inspect_err(|err| error!("Failed to monitor job: {err}"))?;

            // Fetch output, only if it ever successfully launched.
            if started_running {
                match cloud.fetch_output(&job_name, &tempdir).await {
                    Ok(output_path) => {
                        info!(
                            "Job completed successfully. Output available at: {}",
                            output_path.display()
                        );
                    }
                    Err(err) => {
                        error!("Failed to fetch output: {err}");
                        return Err(err);
                    }
                }
            } else {
                info!("Job did not start running.");
                return Err(Error::JobDidNotStart);
            }
        }

        Commands::List {
            json,
            verbose,
            since,
        } => {
            // TODO: Maybe just make since optional?
            let since = humantime::parse_duration(since).map_err(|err| {
                error!("Failed to parse duration {since:?}: {err}");
                Error::Argument(err.to_string())
            })?;
            let since = OffsetDateTime::now_utc()
                .checked_sub(since.try_into().unwrap())
                .unwrap();
            let mut jobs = cloud.list_jobs(Some(since)).await?;
            jobs.sort_by(|a, b| {
                a.created_at
                    .cmp(&b.created_at)
                    .then(a.job_name.cmp(&b.job_name))
            });
            if *json {
                println!("{}", serde_json::to_string_pretty(&jobs).unwrap());
            } else {
                for description in jobs {
                    if *verbose {
                        println!("{description:#?}");
                    } else if let Some(job_name) = &description.job_name {
                        print!(
                            "Run {run_id} shard {shard_k} status {status}",
                            run_id = job_name.run_id,
                            shard_k = job_name.shard_k,
                            status = description.status,
                        );
                        if let Some(duration) = description.duration() {
                            print!(" duration {}", shorttime::format(duration));
                        } else if let Some(elapsed) = description.elapsed() {
                            print!(" elapsed {}", shorttime::format(elapsed));
                        }
                        println!();
                    } else {
                        println!(
                            "Unrecognized job {cloud_job_id} status {status}",
                            cloud_job_id = description.cloud_job_id,
                            status = description.status
                        );
                    }
                }
            }
        }

        Commands::Kill { run_id, all } => {
            let what = if *all {
                KillTarget::All
            } else {
                KillTarget::ById(run_id.to_owned())
            };
            cloud.kill(what).await?;
        }

        Commands::ConfigSchema {} => {
            println!(
                "{}",
                serde_json::to_string_pretty(&schema_for!(Config)).unwrap()
            );
        }
    };
    Ok(())
}

async fn monitor_job(cloud: &dyn Cloud, job_id: &CloudJobId) -> Result<(JobStatus, bool)> {
    let mut last_status: Option<JobStatus> = None;
    let mut log_tail = None;
    let mut _logs_ended = false;
    let mut started_running = false;
    loop {
        let job_description = cloud.describe_job(job_id).await?;
        let status = job_description.status;
        if last_status != Some(status) {
            info!(?job_id, "Job status changed to {status}");
            if let Some(reason) = &job_description.status_reason {
                info!(?job_id, "Status reason: {reason}");
            }
            last_status = Some(job_description.status);
        }
        match job_description.status {
            JobStatus::Completed | JobStatus::Failed => {
                // TODO: Maybe wait just a little longer for the last logs? But, we don't seem to reliably detect the end of the logs.
                return Ok((job_description.status, started_running));
            }
            JobStatus::Running => {
                started_running = true;
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
async fn tar_source(source: &Path, temp_dir: &Path) -> Result<PathBuf> {
    // TODO: Maybe do this in memory to avoid dependencies on the system tar, so that
    // we can exclude /target without false positives and without failing on non-GNU
    // tars? But, we still need to use the system tar in the workers...
    debug!("Tarring source directory...");
    let tarball_path = temp_dir.join(SOURCE_TARBALL_NAME);
    let mut child = Command::new("tar")
        .arg("--zstd")
        .arg("-cf")
        .arg(&tarball_path)
        .arg("-C")
        .arg(source)
        .arg("--exclude-caches") // should get /target without false positives?
        // .arg("--exclude")
        // .arg("target")
        .arg(".")
        .spawn()
        .map_err(Error::Io)?;
    let exit_status = child.wait().await.map_err(Error::Io)?;
    if !exit_status.success() {
        return Err(Error::Tar(format!("tar failed: {exit_status}")));
    }
    debug!("Source tarball size: {}", tarball_path.metadata()?.len());
    Ok(tarball_path)
}

fn setup_tracing(temp_path: &Path) {
    let log_path = temp_path.join(format!("{TOOL_NAME}-debug.log",));
    let log_file = File::create(&log_path).unwrap();
    let file_layer = fmt::Layer::new()
        .with_ansi(false)
        // .with_file(true)
        // .with_line_number(true)
        .with_target(true)
        .with_level(true)
        .with_writer(log_file)
        .with_filter(filter_fn(|metadata| {
            // AWS SDK logs are very verbose so we only want to see our own logs for now;
            // maybe we should have a separate log file for them?
            metadata.target().starts_with("mutants_remote")
        }))
        .with_filter(LevelFilter::DEBUG);
    let stderr_layer = fmt::Layer::new()
        .with_target(true)
        .with_level(true)
        .with_writer(std::io::stderr)
        .with_filter(filter_fn(|metadata| {
            metadata.target().starts_with("mutants_remote")
        }))
        .with_filter(LevelFilter::INFO); // EnvFilter::from_default_env());
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry()
            .with(stderr_layer)
            .with(file_layer),
    )
    .unwrap();
    info!("Tracing initialized to file {}", log_path.display());
}
