//! Launch cargo-mutants into AWS Batch jobs.

use std::time::{Duration, SystemTime};

use aws_config::{BehaviorVersion, meta::region::RegionProviderChain};
use aws_sdk_batch::types::{
    EcsPropertiesOverride, JobStatus, TaskContainerOverrides, TaskPropertiesOverride,
};
use log_tail::LogTail;
use tokio::time::sleep;
#[allow(unused_imports)]
use tracing::{debug, error, info, trace, warn};
use tracing_subscriber::{filter::EnvFilter, fmt, layer::SubscriberExt};

mod log_tail;

#[tokio::main]
async fn main() {
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry()
            .with(fmt::layer())
            .with(EnvFilter::from_default_env()),
    )
    .unwrap();
    let bucket = "mutants-tmp-0733-uswest2";
    // region="us-west-2"
    // let compute_environment = "mutants0-amd64";
    let queue = "mutants0-amd64";
    let job_def = "mutants0-amd64";
    let tarball_id = "d2af2b92-a8bc-495d-a2d4-0fce10830929"; //  "e1013963-8d90-458a-a42f-950ab6271e31";
    let log_group_name = "/aws/batch/job";
    // job_id="$(date -Iminutes)-$SRANDOM"
    // tmp=$(mktemp -d)

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
    let job_name = format!(
        "{time}-{random}",
        time = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        random = fastrand::u32(..)
    );

    let command = format!(
        "yum install -y tar zstd rustc cargo clang awscli &&
        aws s3 cp s3://{bucket}/{tarball_id}/mutants.tar.zst /tmp/mutants.tar.zst &&
        mkdir /work &&
        cd /work &&
        cargo install cargo-nextest &&
        tar xf /tmp/mutants.tar.zst --zstd &&
        cargo test --all --all-features"
    );

    // aws batch submit-job --job-name "$job_id" --job-queue "$queue" --job-definition "$job_def" \
    //     --region "$region" \
    //     --ecs-properties-override "taskPropertiescommand=[\"bash\",\"-c\",\"$script\"]"
    // TODO: Maybe override the log group name.
    info!("Submitting job");
    let task_container_overrides = TaskContainerOverrides::builder()
        .set_command(Some(vec!["bash".to_owned(), "-c".to_owned(), command]))
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
        .job_name(job_name)
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
        // TODO: Should we actually even wait a little longer for them all to arrive?
        if let Some(log_tail) = &mut log_tail {
            for event in log_tail.get_log_events().await {
                println!("    {}", event.message().unwrap_or_default());
            }
        }
        match job_detail.status().unwrap() {
            JobStatus::Succeeded => {
                info!("Job succeeded!");
                break;
            }
            JobStatus::Failed => {
                error!("Job failed!");
                return;
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
        // info!(?container_properties);
    }
}
