// Copyright 2025 Martin Pool

//! AWS Batch implementation of the Cloud abstraction.

#[allow(unused_imports)]
use std::error::Error as StdError;

use std::{
    fmt::Debug,
    path::{Path, PathBuf},
};

use async_trait::async_trait;
use aws_config::{BehaviorVersion, meta::region::RegionProviderChain};
use aws_sdk_batch::{
    error::SdkError,
    types::{
        EcsPropertiesOverride, JobStatus as AwsJobStatus, KeyValuesPair, TaskContainerOverrides,
        TaskPropertiesOverride,
    },
};
use aws_sdk_cloudwatchlogs::primitives::event_stream::EventReceiver;
use aws_sdk_cloudwatchlogs::types::StartLiveTailResponseStream;
use aws_sdk_cloudwatchlogs::types::error::StartLiveTailResponseStreamError;
use aws_sdk_s3::primitives::ByteStream;
use bytes::Bytes;
use time::OffsetDateTime;
use tracing::{debug, error, info, warn};

use super::{Cloud, CloudJobId, LogTail, RUN_ID_TAG};
use crate::job::{JobDescription, JobName, JobStatus};
use crate::{Error, Result, RunId, SOURCE_TARBALL_NAME, TOOL_NAME};

pub struct AwsCloud {
    account_id: String,
    sdk_config: aws_config::SdkConfig,
    batch_client: aws_sdk_batch::Client,
    s3_client: aws_sdk_s3::Client,
    logs_client: aws_sdk_cloudwatchlogs::Client,
    s3_bucket_name: String,
    job_queue_name: String,
    job_definition_name: String,
    log_group_name: String,
}

impl AwsCloud {
    pub async fn new(config: crate::Config) -> Result<Self> {
        let region_provider = RegionProviderChain::default_provider().or_else("us-east-1");
        let sdk_config = aws_config::defaults(BehaviorVersion::latest())
            .region(region_provider)
            .app_name(
                aws_config::AppName::new(format!(
                    "{}-{}",
                    env!("CARGO_PKG_NAME"),
                    env!("CARGO_PKG_VERSION")
                ))
                .unwrap(),
            )
            .load()
            .await;

        let sts_client = aws_sdk_sts::Client::new(&sdk_config);
        let batch_client = aws_sdk_batch::Client::new(&sdk_config);
        let s3_client = aws_sdk_s3::Client::new(&sdk_config);
        let logs_client = aws_sdk_cloudwatchlogs::Client::new(&sdk_config);

        // TODO: Better auto name including the account id.
        let s3_bucket_name = config
            .aws_s3_bucket
            .clone()
            .unwrap_or("mutants-tmp-0733-uswest2".to_string());

        let job_queue_name = config
            .aws_batch_job_queue
            .clone()
            .unwrap_or("mutants0-amd64".to_string());

        let job_definition_name = config
            .aws_batch_job_definition
            .clone()
            .unwrap_or("mutants0-amd64".to_string());

        let log_group_name = config
            .aws_log_group_name
            .clone()
            .unwrap_or("/aws/batch/job".to_string());

        let account_id = match sts_client
            .get_caller_identity()
            .send()
            .await
            .inspect_err(|err| {
                error!(?err, "GetCallerIdentity failed");
            }) {
            Err(aws_sdk_sts::error::SdkError::DispatchFailure(dispatch)) => {
                error!(
                    ?dispatch,
                    is_io = dispatch.is_io(),
                    is_user = dispatch.is_user(),
                    is_other = dispatch.is_other(),
                    is_timeout = dispatch.is_timeout(),
                    "DispatchFailure"
                );
                Err(Error::Credentials(format!("{dispatch:?}")))
            }
            Err(err) => Err(err.into()),
            Ok(caller_identity) => {
                let account_id = caller_identity.account().unwrap().to_owned();
                debug!(?account_id);
                Ok(account_id)
            }
        }?;

        Ok(Self {
            account_id,
            sdk_config,
            batch_client,
            s3_client,
            logs_client,
            s3_bucket_name,
            job_queue_name,
            job_definition_name,
            log_group_name,
        })
    }

    /// Return the URL for an object inside the configured S3 bucket.
    fn s3_url(&self, key: &str) -> String {
        format!("s3://{}/{}", self.s3_bucket_name, key)
    }

    fn run_prefix(&self, run_id: &RunId) -> String {
        format!("{TOOL_NAME}/{run_id}")
    }

    fn job_name_prefix(&self, job_name: &JobName) -> String {
        format!(
            "{TOOL_NAME}/{run_id}/shard-{shard_k}",
            run_id = job_name.run_id,
            shard_k = job_name.shard_k
        )
    }

    fn source_tarball_key(&self, run_id: &RunId) -> String {
        format!("{}/{}", self.run_prefix(run_id), SOURCE_TARBALL_NAME)
    }

    fn source_tarball_s3_url(&self, run_id: &RunId) -> String {
        self.s3_url(&self.source_tarball_key(run_id))
    }

    /// The S3 key for the output tarball for a job.
    fn output_tarball_key(&self, job_name: &JobName) -> String {
        format!(
            "{}/{}",
            self.job_name_prefix(job_name),
            super::OUTPUT_TARBALL_NAME
        )
    }

    /// The S3 URL for the output tarball for a job.
    fn output_tarball_s3_url(&self, job_name: &JobName) -> String {
        self.s3_url(&self.output_tarball_key(job_name))
    }
}

#[async_trait]
impl Cloud for AwsCloud {
    async fn upload_source_tarball(&self, run_id: &RunId, source_tarball: &Path) -> Result<()> {
        debug!("Uploading source tarball to S3");
        let source_tarball_body = ByteStream::from_path(source_tarball).await.map_err(|err| {
            error!(
                "Failed to read source tarball {}: {err}",
                source_tarball.display()
            );
            Error::Cloud(err.into())
        })?;
        self.s3_client
            .put_object()
            .bucket(&self.s3_bucket_name)
            .key(self.source_tarball_key(run_id))
            .body(source_tarball_body)
            .tagging(format!("{RUN_ID_TAG}={run_id}"))
            .send()
            .await?;
        Ok(())
    }

    async fn submit_job(&self, job_name: &JobName, script: String) -> Result<CloudJobId> {
        // Because AWS has modest limits on the length of the size of the job overrides we upload
        // the script to S3 and then fetch that object.
        let run_id = &job_name.run_id;
        let script_key = format!("{}/script.sh", self.run_prefix(run_id));
        let source_tarball_s3_url = self.source_tarball_s3_url(run_id);
        let output_tarball_url = self.output_tarball_s3_url(job_name);

        let wrapped_script = format!(
            "
            aws s3 cp {source_tarball_s3_url} /tmp/mutants.tar.zst &&
            mkdir /work &&
            cd /work &&
            tar xf /tmp/mutants.tar.zst --zstd &&
            {script}
            tar cf /tmp/mutants.out.tar.zstd mutants.out --zstd &&
            aws s3 cp /tmp/mutants.out.tar.zstd {output_tarball_url}
            ",
        );

        self.s3_client
            .put_object()
            .key(script_key.clone())
            .tagging(format!("{RUN_ID_TAG}={run_id}"))
            .body(ByteStream::from(Bytes::from(wrapped_script)))
            .bucket(&self.s3_bucket_name)
            .send()
            .await?;
        let script_s3_url = self.s3_url(&script_key);
        let full_command = format!(
            "aws s3 cp {script_s3_url} /tmp/script.sh &&
            bash -ex /tmp/script.sh
            ",
        );
        debug!(?script_s3_url, "Uploading script to S3");

        info!("Submitting job");
        let task_container_overrides = TaskContainerOverrides::builder()
            .set_command(Some(vec![
                "bash".to_owned(),
                "-exc".to_owned(),
                full_command,
            ]))
            .set_name(Some("root".to_owned())) // container name
            .build();
        let task_properties_overrides = TaskPropertiesOverride::builder()
            .containers(task_container_overrides)
            .build();
        let ecs_properties_overrides = EcsPropertiesOverride::builder()
            .task_properties(task_properties_overrides)
            .build();

        // TODO: Also a job id tag?
        let result = self
            .batch_client
            .submit_job()
            .job_name(job_name.to_string())
            .job_queue(&self.job_queue_name)
            .job_definition(&self.job_definition_name)
            .tags(RUN_ID_TAG, run_id.to_string())
            .propagate_tags(true)
            .ecs_properties_override(ecs_properties_overrides)
            .send()
            .await?;

        let job_id = result.job_id().unwrap().to_owned();
        info!(?job_id, "Job submitted successfully: {:?}", result);
        Ok(CloudJobId(job_id))
    }

    async fn describe_job(&self, job_id: &CloudJobId) -> Result<JobDescription> {
        let description = self
            .batch_client
            .describe_jobs()
            .jobs(job_id.0.clone())
            .send()
            .await
            .map_err(|e| {
                error!("Describe job failed: {}", e);
                Error::Cloud(e.into())
            })?;
        debug!(?job_id, ?description, "Job description");
        let job_detail = description.jobs().first().expect("Job detail");
        let ecs_properties = job_detail.ecs_properties().expect("ecs_properties");
        assert_eq!(
            ecs_properties.task_properties().len(),
            1,
            "Expected exactly one task"
        );
        let container_properties = &ecs_properties.task_properties()[0].containers()[0];
        let log_stream_name = container_properties
            .log_stream_name()
            .map(ToOwned::to_owned);
        let raw_job_name = job_detail.job_name().map(ToOwned::to_owned);
        let job_name = raw_job_name.as_ref().and_then(|n| {
            n.parse()
                .inspect_err(|err| warn!(?raw_job_name, ?err, "Failed to parse job name"))
                .ok()
        });
        Ok(JobDescription {
            cloud_job_id: job_id.clone(),
            status: JobStatus::from(description.jobs()[0].status().unwrap().to_owned()),
            log_stream_name,
            raw_job_name,
            job_name,
            started_at: from_unix_millis(job_detail.started_at),
            stopped_at: from_unix_millis(job_detail.stopped_at),
        })
    }

    async fn list_jobs(&self) -> Result<Vec<JobDescription>> {
        // TODO: Maybe filter jobs, perhaps with a default cutoff some time before now?
        // TODO: Maybe filter to one queue?
        debug!(job_queue_name = ?self.job_queue_name, "Listing jobs");
        // We must set a filter to get jobs in all states.
        let cutoff_time = OffsetDateTime::now_utc() - time::Duration::days(7);
        let cutoff_unix_millis = cutoff_time.unix_timestamp() * 1000;
        let filters = KeyValuesPair::builder()
            .name("AFTER_CREATED_AT")
            .values(cutoff_unix_millis.to_string())
            .build();
        let list_jobs_builder = self
            .batch_client
            .list_jobs()
            .filters(filters)
            .job_queue(&self.job_queue_name);
        let stream = list_jobs_builder.into_paginator().items().send();
        let summaries = stream.collect::<Vec<_>>().await;
        debug!("Received {} job summaries", summaries.len());
        let mut jobs = Vec::new();
        for summary in summaries {
            // TODO: We could actually describe them in batches of up to 100 jobs at a time
            let summary = summary.inspect_err(|err| error!(?err, "list_jobs error"))?;
            // To get the tags, which will let us understand what the job is doing, we have to describe the job
            let cloud_job_id = CloudJobId(summary.job_id.expect("job_id"));
            let description = self
                .describe_job(&cloud_job_id)
                .await
                .inspect_err(|err| error!(?err, "describe_job error"))?;
            jobs.push(description);
        }
        info!("{} jobs listed", jobs.len());
        Ok(jobs)
    }

    async fn fetch_output(&self, job_name: &JobName) -> Result<PathBuf> {
        let output_tarball_key = self.output_tarball_key(job_name);

        let output_tarball = self
            .s3_client
            .get_object()
            .bucket(&self.s3_bucket_name)
            .key(&output_tarball_key)
            .send()
            .await?;

        let local_output_tarball_path =
            std::env::temp_dir().join(format!("mutants-remote-output-{job_name}.tar.zstd",));
        let body = output_tarball.body.collect().await.unwrap().to_vec();
        tokio::fs::write(&local_output_tarball_path, body)
            .await
            .map_err(|err| Error::on_path(err, &local_output_tarball_path))?;
        info!("Output fetched to {}", local_output_tarball_path.display());

        Ok(local_output_tarball_path)
    }

    async fn tail_log(&self, job_description: &JobDescription) -> Result<Box<dyn LogTail>> {
        // TODO: Maybe return None if the description doesn't have a log stream name yet?
        let log_tail = AwsLogTail::new(
            self,
            &self.log_group_name,
            job_description.log_stream_name.as_ref().unwrap(),
        )
        .await;
        Ok(Box::new(log_tail))
    }
}

impl From<AwsJobStatus> for JobStatus {
    fn from(status: AwsJobStatus) -> Self {
        match status {
            // A constraint here is that the log stream will only exist once the job becomes Running
            // so we treat everything else as pending.
            AwsJobStatus::Pending => JobStatus::Pending,
            AwsJobStatus::Runnable => JobStatus::Runnable,
            AwsJobStatus::Submitted => JobStatus::Submitted,
            AwsJobStatus::Starting => JobStatus::Starting,
            AwsJobStatus::Running => JobStatus::Running,
            AwsJobStatus::Succeeded => JobStatus::Completed,
            AwsJobStatus::Failed => JobStatus::Failed,
            _ => JobStatus::Unknown,
        }
    }
}

#[derive(Debug)]
struct AwsLogTail {
    #[allow(dead_code)] // just for debugging?
    pub log_stream_name: String,
    response_stream: EventReceiver<StartLiveTailResponseStream, StartLiveTailResponseStreamError>,
}

impl AwsLogTail {
    async fn new(aws_cloud: &AwsCloud, log_group_name: &str, log_stream_name: &str) -> Self {
        assert_ne!(log_stream_name, "");
        assert_ne!(log_group_name, "");
        let log_group_arn = log_group_arn(aws_cloud, log_group_name);
        // TODO: Might need to renew the tail after it times out, after 3h.
        debug!(?log_stream_name, ?log_group_arn, "Starting live tail");
        let live_tail = aws_cloud
            .logs_client
            .start_live_tail()
            .log_group_identifiers(log_group_arn.to_owned()) // TODO: Maybe should be the ARN?
            .log_stream_names(log_stream_name.to_owned())
            .send()
            .await
            .unwrap();
        let response_stream = live_tail.response_stream;

        AwsLogTail {
            // logs_client,
            // log_group_name: log_group_name.to_string(),
            log_stream_name: log_stream_name.to_string(),
            response_stream,
        }
    }
}

#[async_trait]
impl crate::cloud::LogTail for AwsLogTail {
    /// Get one page of log events.
    // TODO: Maybe return message times too?
    async fn more_log_events(&mut self) -> Result<Option<Vec<String>>> {
        // info!("Fetching log events from CloudWatch Logs");
        loop {
            match self.response_stream.recv().await {
                Ok(None) => {
                    info!("Log stream ended");
                    return Ok(None);
                }
                Ok(Some(StartLiveTailResponseStream::SessionStart(start))) => {
                    debug!(?start, "Starting live tailing");
                    continue;
                }
                Ok(Some(StartLiveTailResponseStream::SessionUpdate(update))) => {
                    if update.session_metadata().is_some_and(|m| m.sampled) {
                        warn!(?update, "Logs were sampled");
                    }
                    return Ok(Some(
                        update
                            .session_results()
                            .iter()
                            .filter_map(|e| e.message.clone())
                            .collect(),
                    ));
                }
                Ok(Some(unknown)) => {
                    warn!(?unknown, "Unknown live tailing response");
                    continue;
                }
                Err(err) => {
                    error!("Error fetching log events: {}", err);
                    return Err(err.into());
                }
            }
        }
    }
}

fn log_group_arn(aws_cloud: &AwsCloud, log_group_name: &str) -> String {
    format!(
        "arn:aws:logs:{}:{}:log-group:{}",
        aws_cloud.sdk_config.region().unwrap(),
        aws_cloud.account_id,
        log_group_name
    )
}

impl<R: Debug + Send + Sync + 'static, E: std::error::Error + Sync + Send + 'static>
    From<SdkError<E, R>> for Error
{
    fn from(err: SdkError<E, R>) -> Self {
        match err {
            // Unpack this because the Display form of the service error is much more informative,
            // including the actual message from the service. The top level error is often just
            // "service error".
            SdkError::ServiceError(service_error) => {
                Error::Cloud(Box::new(service_error.into_err()))
            }
            _ => Error::Cloud(Box::new(err)),
        }
    }
}

/// Convert from unix millis, treating 0 as unknown.
fn from_unix_millis(t: Option<i64>) -> Option<OffsetDateTime> {
    match t {
        Some(0) | None => None,
        Some(t) => OffsetDateTime::from_unix_timestamp_nanos(t as i128 * 1_000_000).ok(),
    }
}
