//! AWS Batch implementation of the Cloud abstraction.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use aws_config::{BehaviorVersion, meta::region::RegionProviderChain};
use aws_sdk_batch::types::{
    EcsPropertiesOverride, JobStatus as AwsJobStatus, TaskContainerOverrides,
    TaskPropertiesOverride,
};
use aws_sdk_cloudwatchlogs::primitives::event_stream::EventReceiver;
use aws_sdk_cloudwatchlogs::types::StartLiveTailResponseStream;
use aws_sdk_cloudwatchlogs::types::error::StartLiveTailResponseStreamError;
use aws_sdk_s3::primitives::ByteStream;
use bytes::Bytes;
use tracing::{debug, error, info, warn};

use super::{Cloud, CloudError, CloudJobId, JobDescription, LogTail, SUITE_ID_TAG};
use crate::{JobStatus, SOURCE_TARBALL_NAME, Suite, TOOL_NAME};

pub struct AwsCloud {
    account_id: String,
    sdk_config: aws_config::SdkConfig,
    batch_client: aws_sdk_batch::Client,
    s3_client: aws_sdk_s3::Client,
    logs_client: aws_sdk_cloudwatchlogs::Client,
    /// Description of the suite being run.
    // TODO: Does this really belong in the cloud or should it be passed in to methods?
    suite: Suite,
}

impl AwsCloud {
    pub async fn new(suite: Suite) -> Result<Self, CloudError> {
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

        let caller_identity = sts_client
            .get_caller_identity()
            .send()
            .await
            .map_err(|e| CloudError::Provider(e.into()))?;
        let account_id = caller_identity.account().unwrap().to_owned();
        debug!(?account_id);

        Ok(Self {
            account_id,
            sdk_config,
            batch_client,
            s3_client,
            logs_client,
            suite,
        })
    }

    fn s3_key_prefix(&self) -> String {
        format!("{TOOL_NAME}/{suite_id}", suite_id = self.suite.suite_id)
    }

    fn source_tarball_key(&self) -> String {
        format!("{}/{}", self.s3_key_prefix(), SOURCE_TARBALL_NAME)
    }

    fn source_tarball_s3_url(&self) -> String {
        format!(
            "s3://{}/{}",
            self.suite.config.aws_s3_bucket,
            self.source_tarball_key()
        )
    }

    fn output_tarball_key(&self) -> String {
        format!("{}/{}", self.s3_key_prefix(), super::OUTPUT_TARBALL_NAME)
    }

    fn output_tarball_s3_url(&self) -> String {
        format!(
            "s3://{}/{}",
            self.suite.config.aws_s3_bucket,
            self.output_tarball_key()
        )
    }
}

#[async_trait]
impl Cloud for AwsCloud {
    async fn upload_source_tarball(&self, source_tarball: &Path) -> Result<(), CloudError> {
        debug!("Uploading source tarball to S3");
        let source_tarball_body = ByteStream::from_path(source_tarball)
            .await
            .map_err(|e| CloudError::Provider(e.into()))?;
        self.s3_client
            .put_object()
            .bucket(&self.suite.config.aws_s3_bucket)
            .key(self.source_tarball_key())
            .body(source_tarball_body)
            .tagging(format!("{SUITE_ID_TAG}={}", self.suite.suite_id))
            .send()
            .await
            .map_err(|e| CloudError::Provider(e.into()))?;
        Ok(())
    }

    async fn submit_job(&self, script: String, job_name: String) -> Result<CloudJobId, CloudError> {
        // Because AWS has modest limits on the length of the size of the job overrides we upload
        // the script to S3 and then fetch that object.
        let script_key = format!("{}/script.sh", self.suite.suite_id);
        let output_tarball_url = self.output_tarball_s3_url();

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
            source_tarball_s3_url = self.source_tarball_s3_url(),
            output_tarball_url = output_tarball_url,
        );

        self.s3_client
            .put_object()
            .key(script_key.clone())
            .tagging(format!("{SUITE_ID_TAG}={}", self.suite.suite_id))
            .body(ByteStream::from(Bytes::from(wrapped_script)))
            .bucket(&self.suite.config.aws_s3_bucket)
            .send()
            .await
            .map_err(|e| CloudError::Provider(e.into()))?;
        let script_key = format!("{}/script.sh", self.suite.suite_id);
        let script_s3_url = format!(
            "s3://{bucket}/{script_key}",
            bucket = self.suite.config.aws_s3_bucket,
        );
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
            .job_name(job_name)
            .job_queue(&self.suite.config.aws_batch_job_queue)
            .job_definition(&self.suite.config.aws_batch_job_definition)
            .tags(SUITE_ID_TAG, self.suite.suite_id.to_string())
            .propagate_tags(true)
            .ecs_properties_override(ecs_properties_overrides)
            .send()
            .await
            .map_err(|e| CloudError::Provider(e.into()))?;

        let job_id = result.job_id().unwrap().to_owned();
        info!(?job_id, "Job submitted successfully: {:?}", result);
        Ok(CloudJobId(job_id))
    }

    async fn describe_job(&self, job_id: &CloudJobId) -> Result<JobDescription, CloudError> {
        let description = self
            .batch_client
            .describe_jobs()
            .jobs(job_id.0.clone())
            .send()
            .await
            .map_err(|e| {
                error!("Describe job failed: {}", e);
                CloudError::Provider(e.into())
            })?;
        let ecs_properties = description.jobs()[0]
            .ecs_properties()
            .expect("ecs_properties");
        assert_eq!(
            ecs_properties.task_properties().len(),
            1,
            "Expected exactly one task"
        );
        let container_properties = &ecs_properties.task_properties()[0].containers()[0];
        let log_stream_name = container_properties
            .log_stream_name()
            .map(ToOwned::to_owned);
        Ok(JobDescription {
            job_id: job_id.clone(),
            status: JobStatus::from(description.jobs()[0].status().unwrap().to_owned()),
            log_stream_name,
        })
    }

    async fn fetch_output(&self, job_id: &CloudJobId) -> Result<PathBuf, CloudError> {
        let output_tarball_key = self.output_tarball_key();

        let output_tarball = self
            .s3_client
            .get_object()
            .bucket(&self.suite.config.aws_s3_bucket)
            .key(&output_tarball_key)
            .send()
            .await
            .map_err(|e| CloudError::Provider(e.into()))?;

        let output_tarball_path = std::env::temp_dir().join(format!(
            "mutants-remote-output-{suite_id}-{job_id}.tar.zstd",
            suite_id = self.suite.suite_id,
            job_id = job_id.0
        ));
        let body = output_tarball.body.collect().await.unwrap().to_vec();
        tokio::fs::write(&output_tarball_path, body)
            .await
            .map_err(CloudError::Io)?;
        info!("Output fetched to {}", output_tarball_path.display());

        Ok(output_tarball_path)
    }

    async fn tail_log(
        &self,
        job_description: &JobDescription,
    ) -> Result<Box<dyn LogTail>, CloudError> {
        // TODO: Maybe return None if the description doesn't have a log stream name yet?
        let log_tail = AwsLogTail::new(
            self,
            &self.suite.config.aws_log_group_name,
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
    async fn more_log_events(&mut self) -> Result<Option<Vec<String>>, CloudError> {
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
                    return Err(CloudError::Provider(err.into()));
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
