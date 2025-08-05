//! Cloud abstraction for mutants-remote.
//!
//! Provides an aspirationally generic interface for cloud providers: the core features are to get and put files, launch jobs, and monitor the status of jobs.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use aws_config::{BehaviorVersion, meta::region::RegionProviderChain};
use aws_sdk_batch::types::{
    EcsPropertiesOverride, JobStatus, TaskContainerOverrides, TaskPropertiesOverride,
};
use aws_sdk_s3::primitives::ByteStream;
use bytes::Bytes;
use thiserror::Error;
use tokio::time::{Instant, sleep};
use tracing::{debug, error, info};

use crate::log_tail::LogTail;
use crate::{SOURCE_TARBALL_NAME, Suite};

static SUITE_ID_TAG: &str = "mutants-remote-suite";
static OUTPUT_TARBALL_NAME: &str = "mutants.out.tar.zstd";

#[derive(Error, Debug)]
pub enum CloudError {
    #[error("AWS S3 error: {0}")]
    S3(#[from] aws_sdk_s3::Error),

    #[error("AWS Batch error: {0}")]
    Batch(#[from] aws_sdk_batch::Error),

    #[error("AWS STS error: {0}")]
    Sts(#[from] aws_sdk_sts::Error),

    #[error("AWS CloudWatch Logs error: {0}")]
    CloudWatchLogs(#[from] aws_sdk_cloudwatchlogs::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("ByteStream error: {0}")]
    ByteStream(#[from] aws_sdk_s3::primitives::ByteStreamError),
}

#[async_trait]
pub trait Cloud {
    async fn upload_source_tarball(&self, source_tarball: &Path) -> Result<(), CloudError>;
    async fn submit_job(&self, script: String, job_name: String) -> Result<CloudJobId, CloudError>;
    async fn monitor_job(&self, job_id: &CloudJobId) -> Result<(), CloudError>;
    async fn fetch_output(&self, job_id: &CloudJobId) -> Result<PathBuf, CloudError>;
}

pub struct AwsCloud {
    sdk_config: aws_config::SdkConfig,
    batch_client: aws_sdk_batch::Client,
    s3_client: aws_sdk_s3::Client,
    account_id: String,
    suite: Suite,
}

/// The identifier for a job assigned by the cloud.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CloudJobId(String);

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

        let caller_identity = sts_client
            .get_caller_identity()
            .send()
            .await
            .map_err(|e| CloudError::Sts(e.into()))?;
        let account_id = caller_identity.account().unwrap().to_owned();
        debug!(?account_id);

        Ok(Self {
            sdk_config,
            batch_client,
            s3_client,
            account_id,
            suite,
        })
    }

    fn source_tarball_key(&self) -> String {
        format!("{}/{}", self.suite.suite_id, SOURCE_TARBALL_NAME)
    }

    fn source_tarball_s3_url(&self) -> String {
        format!(
            "s3://{}/{}",
            self.suite.config.aws_s3_bucket,
            self.source_tarball_key()
        )
    }

    fn output_tarball_key(&self) -> String {
        format!("{}/{}", self.suite.suite_id, OUTPUT_TARBALL_NAME)
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
            .map_err(CloudError::ByteStream)?;
        self.s3_client
            .put_object()
            .bucket(&self.suite.config.aws_s3_bucket)
            .key(self.source_tarball_key())
            .body(source_tarball_body)
            .tagging(format!("{SUITE_ID_TAG}={}", self.suite.suite_id))
            .send()
            .await
            .map_err(|e| CloudError::S3(e.into()))?;
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
            .map_err(|e| CloudError::S3(e.into()))?;
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
            .map_err(|e| CloudError::Batch(e.into()))?;

        let job_id = result.job_id().unwrap().to_owned();
        info!(?job_id, "Job submitted successfully: {:?}", result);
        Ok(CloudJobId(job_id))
    }

    async fn monitor_job(&self, job_id: &CloudJobId) -> Result<(), CloudError> {
        let mut last_status: Option<JobStatus> = None;
        let mut log_tail: Option<LogTail> = None;
        let mut ended_at: Option<Instant> = None;

        loop {
            sleep(Duration::from_secs(1)).await;
            let result = self
                .batch_client
                .describe_jobs()
                .jobs(job_id.0.clone())
                .send()
                .await
                .map_err(|e| CloudError::Batch(e.into()))?;

            let job_detail = &result.jobs()[0];
            let status = job_detail.status().unwrap().to_owned();
            if last_status != Some(status.clone()) {
                info!(cloud_job_id = ?job_id.0, "Job status changed to {status}");
                last_status = Some(status);
            }

            // Fetch logs before potentially exiting if the job has stopped
            if let Some(log_tail) = &mut log_tail {
                match log_tail.more_log_events().await {
                    Some(events) => {
                        for message in events {
                            println!("    {message}");
                        }
                    }
                    None => {
                        info!("End of log stream");
                    }
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
                    // Job is running
                }
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
                match log_tail {
                    None => {
                        info!("Starting log tail for stream {log_stream_name}");
                        let log_group_arn = format!(
                            "arn:aws:logs:{}:{}:log-group:{}",
                            self.sdk_config.region().unwrap(),
                            self.account_id,
                            self.suite.config.aws_log_group_name
                        );
                        log_tail = Some(
                            LogTail::new(&self.sdk_config, &log_group_arn, log_stream_name).await,
                        );
                    }
                    Some(ref tail) => assert_eq!(tail.log_stream_name, log_stream_name),
                }
            }

            if ended_at.is_some_and(|e| e.elapsed().as_millis() > 2000) {
                break;
            }
        }

        Ok(())
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
            .map_err(|e| CloudError::S3(e.into()))?;

        let output_tarball_path =
            std::env::temp_dir().join(format!("mutants-remote-output-{}.tar.zstd", job_id.0));
        let body = output_tarball.body.collect().await.unwrap().to_vec();
        tokio::fs::write(&output_tarball_path, body)
            .await
            .map_err(CloudError::Io)?;
        info!("Output fetched to {}", output_tarball_path.display());

        Ok(output_tarball_path)
    }
}
