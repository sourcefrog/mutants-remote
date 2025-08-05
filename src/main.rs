//! Launch cargo-mutants into AWS Batch jobs.

use std::{env::temp_dir, fs::File};
use thiserror::Error;
use tokio;
use tracing::level_filters::LevelFilter;
use tracing::{error, info};
use tracing_subscriber::{Layer, filter::filter_fn, fmt, layer::SubscriberExt};

mod cloud;
mod log_tail;
use crate::cloud::{AwsCloud, Cloud, JobConfig};

const TOOL_NAME: &str = "mutants-remote";

// TODO: Also try `thistermination` to give specific error codes...
#[derive(Error, Debug)]
pub enum Error {
    #[error("Cloud error: {0}")]
    Cloud(#[from] cloud::CloudError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

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

#[tokio::main]
async fn main() -> Result<(), Error> {
    let invocation_id = invocation_id();
    setup_tracing(&invocation_id);

    // TODO: Tar up the source directory, maybe from `-d`.

    // Create job configuration
    let config = JobConfig {
        bucket: "mutants-tmp-0733-uswest2".to_string(),
        queue: "mutants0-amd64".to_string(),
        job_def: "mutants0-amd64".to_string(),
        tarball_id: "d2af2b92-a8bc-495d-a2d4-0fce10830929".to_string(),
        log_group_name: "/aws/batch/job".to_string(),
        output_tarball_name: "mutants.out.tar.zstd".to_string(),
    };

    // Create AWS cloud provider
    let cloud = match AwsCloud::new().await {
        Ok(cloud) => cloud,
        Err(err) => {
            error!("Failed to initialize AWS cloud: {err}");
            return Err(Error::Cloud(err));
        }
    };

    // Upload script
    let script = "cargo mutants --shard 0/100 -vV || true".to_string();
    let script_key = match cloud.upload_script(script, &invocation_id, &config).await {
        Ok(key) => key,
        Err(err) => {
            error!("Failed to upload script: {err}");
            return Err(Error::Cloud(err));
        }
    };

    // Submit job
    let job_name = format!("{TOOL_NAME}-{invocation_id}");
    let job_id = match cloud
        .submit_job(script_key, job_name, &invocation_id, &config)
        .await
    {
        Ok(id) => id,
        Err(err) => {
            error!("Failed to submit job: {err}");
            return Err(Error::Cloud(err));
        }
    };

    // Monitor job
    if let Err(err) = cloud.monitor_job(job_id, &config).await {
        error!("Failed to monitor job: {err}");
        return Err(Error::Cloud(err));
    }

    // Fetch output
    match cloud.fetch_output(&invocation_id, &config).await {
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

fn setup_tracing(job_name: &str) {
    let log_path = temp_dir().join(format!("{TOOL_NAME}-{job_name}.log"));
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

fn invocation_id() -> String {
    let now = chrono::Local::now();
    let time_str = now.format("%Y%m%d%H%M%S").to_string();
    format!(
        "{time}-{random:08x}",
        time = time_str,
        random = fastrand::u32(..)
    )
}
