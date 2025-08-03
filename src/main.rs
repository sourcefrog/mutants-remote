//! Launch cargo-mutants into AWS Batch jobs.

use std::{
    env::temp_dir,
    fs::File,
    time::{Duration, SystemTime},
};

use aws_config::{BehaviorVersion, meta::region::RegionProviderChain};
use aws_sdk_batch::types::{
    EcsPropertiesOverride, JobStatus, TaskContainerOverrides, TaskPropertiesOverride,
};
use aws_sdk_s3::primitives::ByteStream;
use bytes::Bytes;
use tokio::time::{Instant, sleep};
use tracing::level_filters::LevelFilter;
#[allow(unused_imports)]
use tracing::{debug, error, info, trace, warn};
use tracing_subscriber::{
    Layer,
    filter::{EnvFilter, filter_fn},
    fmt,
    layer::SubscriberExt,
};

mod log_tail;
use crate::log_tail::LogTail;

#[tokio::main]
async fn main() {
    let invocation_id = format!(
        "{time}-{random}",
        time = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        random = fastrand::u32(..)
    );
    setup_tracing(&invocation_id);
    let bucket = "mutants-tmp-0733-uswest2";
    // region="us-west-2"
    // let compute_environment = "mutants0-amd64";
    let queue = "mutants0-amd64";
    let job_def = "mutants0-amd64";
    let tarball_id = "d2af2b92-a8bc-495d-a2d4-0fce10830929"; //  "e1013963-8d90-458a-a42f-950ab6271e31";
    let log_group_name = "/aws/batch/job";
    // TODO: Push this into the JobDefinition
    // let image_url = "ghcr.io/sourcefrog/cargo-mutants:container";

    let region_provider = RegionProviderChain::default_provider().or_else("us-east-1");
    let sdk_config = aws_config::defaults(BehaviorVersion::latest())
        .region(region_provider)
        .load()
        .await;
    let batch_client = aws_sdk_batch::Client::new(&sdk_config);
    // let compute_envs = client
    //     .describe_compute_environments()
    //     .send()
    //     .await
    //     .unwrap()
    //     .compute_environments;

    let script = format!(
        "
        aws s3 cp s3://{bucket}/{tarball_id}/mutants.tar.zst /tmp/mutants.tar.zst &&
        mkdir /work &&
        cd /work &&
        cargo install cargo-nextest cargo-mutants &&
        tar xf /tmp/mutants.tar.zst --zstd &&
        cargo mutants --shard 0/100 -vV || true
        tar cfv /tmp/mutants.out.tar.zstd mutants.out --zstd &&
        aws s3 cp /tmp/mutants.out.tar.zstd s3://{bucket}/{tarball_id}/mutants.out.tar.zstd
        "
    );
    let script_key = format!("{invocation_id}/script.sh");

    let s3_client = aws_sdk_s3::Client::new(&sdk_config);
    debug!("Uploading script to S3");
    s3_client
        .put_object()
        .key(script_key.clone())
        .body(ByteStream::from(Bytes::from(script)))
        .bucket(bucket)
        .send()
        .await
        .unwrap();

    let command = format!(
        "aws s3 cp s3://{bucket}/{script_key} /tmp/script.sh &&
        bash -ex /tmp/script.sh
        "
    );

    // TODO: Maybe override the log group name.
    info!("Submitting job");
    let task_container_overrides = TaskContainerOverrides::builder()
        .set_command(Some(vec!["bash".to_owned(), "-exc".to_owned(), command]))
        .set_name(Some("root".to_owned()))
        .build();
    let task_properties_overrides = TaskPropertiesOverride::builder()
        .containers(task_container_overrides)
        .build();
    let ecs_properties_overrides = EcsPropertiesOverride::builder()
        .task_properties(task_properties_overrides)
        .build();
    let job_id;
    match batch_client
        .submit_job()
        .job_name(invocation_id)
        .job_queue(queue)
        .job_definition(job_def)
        .ecs_properties_override(ecs_properties_overrides)
        .send()
        .await
    {
        Ok(result) => {
            job_id = result.job_id().unwrap().to_owned();
            info!(?job_id, "Job submitted successfully: {:?}", result);
        }
        Err(err) => {
            error!("Failed to submit job: {err}");
            return;
        }
    }

    let mut last_status: Option<JobStatus> = None;
    let mut log_tail: Option<LogTail> = None;
    let mut ended_at: Option<Instant> = None;

    loop {
        sleep(Duration::from_secs(1)).await;
        let result = match batch_client
            .describe_jobs()
            .jobs(job_id.clone())
            .send()
            .await
        {
            Ok(result) => {
                // trace!(?result);
                result
            }
            Err(err) => {
                error!("Failed to describe job: {err}");
                return;
            }
        };
        let job_detail = &result.jobs()[0];
        let status = job_detail.status().unwrap().to_owned();
        if last_status != Some(status.clone()) {
            info!(
                "Job status changed to {status} with {ecs} ecs properties and {tasks} tasks",
                ecs = job_detail.ecs_properties().is_some() as u32,
                tasks = job_detail
                    .ecs_properties()
                    .map_or(0, |ecs| ecs.task_properties().len())
            );
            last_status = Some(status);
        }
        // Fetch logs before potentially exiting if the job has stopped, so that we see the final logs.
        // TODO: Keep looking for a couple of seconds before exiting if the job has stopped.
        if let Some(log_tail) = &mut log_tail {
            for event in log_tail.get_log_events().await {
                println!("    {}", event.message().unwrap_or_default());
            }
        }
        match job_detail.status().unwrap() {
            _ if ended_at.is_some() => {}
            JobStatus::Succeeded => {
                info!("Job succeeded!");
                ended_at = Some(Instant::now());
            }
            JobStatus::Failed => {
                error!("Job failed!");
                ended_at = Some(Instant::now());
            }
            JobStatus::Running => {
                // trace!(?job_detail);
                // info!("Job is running");
            }
            // pending, runnable, submitted, and other non-exhaustive options
            _ => continue,
        }
        let ecs_properties = job_detail.ecs_properties().unwrap();
        assert_eq!(
            ecs_properties.task_properties().len(),
            1,
            "Expected exactly one task"
        );
        let container_properties = &ecs_properties.task_properties()[0].containers()[0];
        if let Some(log_stream_name) = container_properties.log_stream_name() {
            // TODO: Maybe spawn it instead of polling?
            // TODO: Maybe use the live tail API.
            match log_tail {
                None => {
                    info!("Starting log tail for stream {log_stream_name}");
                    log_tail = Some(LogTail::new(&sdk_config, log_group_name, log_stream_name));
                }
                Some(ref tail) => assert_eq!(tail.log_stream_name, log_stream_name),
            }
        }
        if ended_at.is_some_and(|e| e.elapsed().as_millis() > 2000) {
            break;
        }
        // info!(?container_properties);
    }
}

fn setup_tracing(job_name: &str) {
    let log_path = temp_dir().join(format!("mutants-batch-{job_name}.log"));
    let log_file = File::create(&log_path).unwrap();
    let file_layer = fmt::Layer::new()
        .with_ansi(false)
        .with_file(true)
        .with_line_number(true)
        .with_target(true)
        .with_level(true)
        .with_writer(log_file)
        .with_filter(filter_fn(|metadata| {
            metadata.target().starts_with("mutants_batch")
        }))
        .with_filter(LevelFilter::DEBUG);
    let stderr_layer = fmt::Layer::new()
        .with_target(true)
        .with_level(true)
        .with_writer(std::io::stderr)
        .with_filter(EnvFilter::from_default_env());
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry()
            .with(stderr_layer)
            .with(file_layer),
    )
    .unwrap();
    info!("Tracing initialized to file {}", log_path.display());
}
