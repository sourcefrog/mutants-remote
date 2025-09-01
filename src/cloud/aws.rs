// Copyright 2025 Martin Pool

//! AWS Batch implementation of the Cloud abstraction.

use std::{
    collections::HashMap,
    fmt::Debug,
    path::{Path, PathBuf},
};

use async_trait::async_trait;
use aws_config::{BehaviorVersion, meta::region::RegionProviderChain};
use aws_sdk_batch::{
    error::SdkError,
    types::{
        EcsPropertiesOverride, JobStatus as AwsJobStatus, KeyValuePair, KeyValuesPair,
        ResourceRequirement, ResourceType, TaskContainerOverrides, TaskPropertiesOverride,
    },
};
use aws_sdk_cloudwatchlogs::primitives::event_stream::EventReceiver;
use aws_sdk_cloudwatchlogs::types::StartLiveTailResponseStream;
use aws_sdk_cloudwatchlogs::types::error::StartLiveTailResponseStreamError;
use aws_sdk_s3::primitives::ByteStream;
use bytes::Bytes;
use jiff::Timestamp;
use tracing::{debug, error, info, trace, warn};

use super::{Cloud, CloudJobId, LogTail};
use crate::{
    Error, Result, SOURCE_TARBALL_NAME, TOOL_NAME, cloud::DEFAULT_BUCKET_PREFIX,
    job::central_command, run::RunTicket, tags::OUTPUT_TARBALL_URL_TAG,
};
use crate::{
    config::Config,
    job::{JobDescription, JobName, JobStatus},
    run::{KillTarget, RunId, RunLabels},
};
use crate::{
    run::RunArgs,
    tags::{MUTANTS_REMOTE_VERSION_TAG, RUN_ID_TAG},
};

#[derive(Debug)]
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
    config: Config,
}

impl AwsCloud {
    pub async fn new(config: Config) -> Result<Self> {
        let region_provider = RegionProviderChain::default_provider().or_else("us-east-1");
        let sdk_config = aws_config::defaults(BehaviorVersion::latest())
            .region(region_provider)
            .app_name(
                aws_config::AppName::new(format!("{}-{}", env!("CARGO_PKG_NAME"), crate::VERSION))
                    .unwrap(),
            )
            .load()
            .await;

        let sts_client = aws_sdk_sts::Client::new(&sdk_config);
        let batch_client = aws_sdk_batch::Client::new(&sdk_config);
        let s3_client = aws_sdk_s3::Client::new(&sdk_config);
        let logs_client = aws_sdk_cloudwatchlogs::Client::new(&sdk_config);

        let s3_bucket_name = match &config.aws_s3_bucket {
            Some(b) => b.clone(),
            None => find_default_bucket(&s3_client).await?,
        };

        let job_queue_name = config
            .aws_batch_job_queue
            .clone()
            .unwrap_or("mutants".to_string());

        let job_definition_name = config
            .aws_batch_job_definition
            .clone()
            .unwrap_or("mutants-amd64".to_string());

        let log_group_name = config
            .aws_log_group_name
            .clone()
            .unwrap_or("mutants-remote".to_string());

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
            config: config.clone(),
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

    /// The S3 key for the output tarball for a job.
    fn output_tarball_key(&self, job_name: &JobName) -> String {
        format!(
            "{}-{}",
            self.job_name_prefix(job_name),
            super::OUTPUT_TARBALL_NAME
        )
    }

    /// The S3 URL for the output tarball for a job.
    fn output_tarball_s3_url(&self, job_name: &JobName) -> String {
        self.s3_url(&self.output_tarball_key(job_name))
    }

    fn s3_tagging(&self, run_id: &RunId) -> String {
        // Just assuming here that none of the values cause quoting issues.
        format!(
            "{RUN_ID_TAG}={run_id}&{MUTANTS_REMOTE_VERSION_TAG}={tool_version}",
            tool_version = crate::VERSION
        )
    }

    async fn put_object(&self, key: &str, content: ByteStream, run_id: &RunId) -> Result<()> {
        debug!(?key, "Upload {} bytes", content.size_hint().0);
        self.s3_client
            .put_object()
            .key(key.to_owned())
            .tagging(self.s3_tagging(run_id))
            .if_none_match("*") // must not exist
            .body(content)
            .bucket(&self.s3_bucket_name)
            .send()
            .await?;
        Ok(())
    }

    async fn upload_source_tarball(&self, run_id: &RunId, source_tarball: &Path) -> Result<String> {
        debug!("Uploading source tarball to S3");
        let source_tarball_body = ByteStream::from_path(source_tarball).await.map_err(|err| {
            error!(
                "Failed to read source tarball {}: {err}",
                source_tarball.display()
            );
            Error::Cloud(err.into())
        })?;
        let key = &self.source_tarball_key(run_id);
        self.put_object(key, source_tarball_body, run_id).await?;
        Ok(self.s3_url(key))
    }

    /// Write a script that can be run by any job to S3.
    ///
    /// Because AWS apparently has modest limits on the length of the size of the job overrides we upload
    /// the script to S3 and then fetch that object.
    async fn upload_script(
        &self,
        run_id: &RunId,
        run_args: &RunArgs,
        source_tarball_s3_url: &str,
    ) -> Result<String> {
        let script = central_command(run_args, "${MUTANTS_REMOTE_SHARD}", run_args.shards);
        let script_key = format!("{}/script.sh", self.run_prefix(run_id));
        let wrapped_script = format!(
            r#"
        free -m && id && pwd && df -m &&
        cd && pwd &&
        export CARGO_HOME=$HOME/.cargo &&
        aws s3 cp {source_tarball_s3_url} /tmp/mutants.tar.zst &&
        mkdir work &&
        cd work &&
        tar xf /tmp/mutants.tar.zst --zstd &&
        {script}
        tar cf /tmp/mutants.out.tar.zstd mutants.out --zstd &&
        aws s3 cp /tmp/mutants.out.tar.zstd ${{MUTANTS_REMOTE_OUTPUT}}
        "#,
        );
        let script_s3_url = self.s3_url(&script_key);
        debug!(?wrapped_script, ?script_s3_url, "Uploading script to S3");
        self.put_object(
            &script_key,
            ByteStream::from(Bytes::from(wrapped_script)),
            run_id,
        )
        .await?;
        Ok(script_s3_url)
    }

    async fn submit_job(
        &self,
        job_name: &JobName,
        run_labels: &RunLabels,
        script_s3_url: &str,
    ) -> Result<CloudJobId> {
        info!(?job_name, "Submitting job");
        let output_tarball_url = self.output_tarball_s3_url(job_name);

        let mut task_container_overrides = TaskContainerOverrides::builder()
        .set_command(Some(vec![
            "bash".to_owned(),
            "-exc".to_owned(),
            format!("aws s3 cp {script_s3_url} /tmp/script.sh && /bin/bash -ex /tmp/script.sh"),
        ]))
        .set_name(Some("mutants-remote".to_owned())) // container name
        ;
        if let Some(vcpus) = self.config.vcpus {
            task_container_overrides = task_container_overrides.resource_requirements(
                ResourceRequirement::builder()
                    .r#type(ResourceType::Vcpu)
                    .value(vcpus.to_string())
                    .build(),
            )
        }
        if let Some(memory) = self.config.memory {
            task_container_overrides = task_container_overrides.resource_requirements(
                ResourceRequirement::builder()
                    .r#type(ResourceType::Memory)
                    .value(memory.to_string())
                    .build(),
            )
        }
        task_container_overrides = task_container_overrides
            .environment(
                KeyValuePair::builder()
                    .name("MUTANTS_REMOTE_SHARD")
                    .value("0")
                    .build(),
            )
            .environment(
                KeyValuePair::builder()
                    .name("MUTANTS_REMOTE_OUTPUT")
                    .value(output_tarball_url.clone())
                    .build(),
            );
        let task_container_overrides = task_container_overrides.build();
        let task_properties_overrides = TaskPropertiesOverride::builder()
            .containers(task_container_overrides)
            .build();
        let ecs_properties_overrides = EcsPropertiesOverride::builder()
            .task_properties(task_properties_overrides)
            .build();

        let mut builder = self
            .batch_client
            .submit_job()
            .job_name(job_name.to_string())
            .job_queue(&self.job_queue_name)
            .job_definition(&self.job_definition_name)
            .tags(RUN_ID_TAG, job_name.run_id.to_string())
            .tags(OUTPUT_TARBALL_URL_TAG, output_tarball_url.clone())
            .propagate_tags(true)
            .ecs_properties_override(ecs_properties_overrides);
        for (key, value) in run_labels.to_tags() {
            builder = builder.tags(key, value);
        }
        let result = builder
            .send()
            .await
            .inspect_err(|err| error!(?err, "SubmitJob failed"))?;
        let cloud_job_id = CloudJobId::new(result.job_id().expect("submitted job id"));
        info!(?cloud_job_id, "Job submitted successfully: {:?}", result);
        Ok(cloud_job_id)
    }

    /// Describe up to 100 jobs in a single call
    async fn describe_jobs(&self, cloud_job_ids: &[&CloudJobId]) -> Result<Vec<JobDescription>> {
        let id_strings = cloud_job_ids
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>();
        trace!(?id_strings, "Describe jobs");
        let description = self
            .batch_client
            .describe_jobs()
            .set_jobs(Some(id_strings))
            .send()
            .await
            .map_err(|e| {
                error!("Describe job failed: {}", e);
                Error::Cloud(e.into())
            })?;
        trace!(?description, "Got description");
        let mut result = Vec::new();
        for job_detail in description.jobs() {
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
            let tags = job_detail
                .tags
                .as_ref()
                .map_or_else(HashMap::new, HashMap::clone);
            let run_labels = Some(RunLabels::from_tags(&tags));
            result.push(JobDescription {
                cloud_job_id: CloudJobId::new(job_detail.job_id().expect("Job ID is required")),
                status: JobStatus::from(job_detail.status()),
                status_reason: job_detail.status_reason().map(ToOwned::to_owned),
                log_stream_name,
                raw_job_name,
                job_name,
                created_at: from_unix_millis(job_detail.created_at),
                started_at: from_unix_millis(job_detail.started_at),
                stopped_at: from_unix_millis(job_detail.stopped_at),
                cloud_tags: job_detail.tags.clone(),
                output_tarball_url: tags.get(OUTPUT_TARBALL_URL_TAG).cloned(),
                run_labels,
            });
        }
        debug_assert_eq!(result.len(), cloud_job_ids.len());
        Ok(result)
    }
}

/// Find the default bucket created by Terraform.
async fn find_default_bucket(s3_client: &aws_sdk_s3::Client) -> Result<String> {
    let response = s3_client.list_buckets().send().await?;
    match response.buckets.unwrap_or_default().iter().find(|bucket| {
        bucket
            .name()
            .unwrap_or_default()
            .starts_with(DEFAULT_BUCKET_PREFIX)
    }) {
        Some(bucket) => {
            let name = bucket.name().expect("Bucket should have a name");
            debug!("Found default bucket: {name}");
            Ok(name.to_string())
        }
        None => Err(Error::Cloud("No default bucket found".into())),
    }
}

#[async_trait]
impl Cloud for AwsCloud {
    async fn submit(
        &self,
        run_id: &RunId,
        run_labels: &RunLabels,
        run_args: &RunArgs,
        source_tarball: &Path,
    ) -> Result<RunTicket> {
        let shards: usize = run_args.shards;
        assert!(shards > 0, "Shard count must be greater than zero");
        let source_tarball_s3_url = self.upload_source_tarball(run_id, source_tarball).await?;
        let script_s3_url = self
            .upload_script(run_id, run_args, &source_tarball_s3_url)
            .await?;

        let mut jobs = Vec::new();
        for shard in 0..shards {
            let job_name = JobName::new(run_id, shard);
            let cloud_job_id = self
                .submit_job(&job_name, run_labels, &script_s3_url)
                .await?;
            jobs.push((job_name, cloud_job_id));
        }

        Ok(RunTicket {
            run_id: run_id.to_owned(),
            jobs,
        })
    }

    async fn describe_job(&self, job_id: &CloudJobId) -> Result<JobDescription> {
        Ok(self.describe_jobs(&[job_id]).await?.remove(0))
    }

    async fn kill(&self, kill_target: KillTarget) -> Result<()> {
        debug!(?kill_target);
        // TODO: Maybe merge into list?
        let jobs_to_kill = self
            .list_jobs(None)
            .await?
            .into_iter()
            .filter(|j| j.status.is_alive())
            .filter(|j| match &kill_target {
                KillTarget::All => true,
                KillTarget::ByRunId(kill_run_ids) => {
                    // Look at either the job name or the tag, to make things more robust if we change naming in the future.
                    j.job_name
                        .as_ref()
                        .is_some_and(|n| kill_run_ids.contains(&n.run_id))
                        || j.cloud_tags.as_ref().is_some_and(|tags| {
                            tags.iter().any(|(key, value)| {
                                key == RUN_ID_TAG
                                    && kill_run_ids
                                        .iter()
                                        .map(ToString::to_string)
                                        .any(|run_id| value == &run_id)
                            })
                        })
                }
            })
            .collect::<Vec<_>>();
        info!("Found {} jobs to kill", jobs_to_kill.len());
        let mut n_killed = 0;
        // TODO: We could send multiple calls in parallel here.
        for job in jobs_to_kill {
            let job_name = job.job_name;
            let cloud_job_id = job.cloud_job_id;
            debug!(?job_name, "Terminating job");
            match self
                .batch_client
                .terminate_job()
                .job_id(cloud_job_id.0.clone())
                .reason("Killed by `mutants-remote kill`")
                .send()
                .await
            {
                Ok(result) => {
                    debug!(?result, ?cloud_job_id, ?job_name, "Terminated job");
                    n_killed += 1;
                }
                Err(err) => warn!(?err, ?cloud_job_id, ?job_name, "Failed to terminate job"), // continue on anyhow, maybe it already terminated
            }
        }
        info!("Terminated {} jobs", n_killed);
        Ok(())
    }

    async fn list_jobs(&self, since: Option<Timestamp>) -> Result<Vec<JobDescription>> {
        // TODO: Maybe filter jobs, perhaps with a default cutoff some time before now?
        // TODO: Maybe filter to one queue?
        debug!(job_queue_name = ?self.job_queue_name, ?since, "Listing jobs");
        // We must set a filter to get jobs in all states.
        let cutoff_unix_millis = since.map_or(0, |since| since.as_millisecond());
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
        let mut cloud_job_ids = Vec::new();
        for summary in summaries {
            let summary = summary.inspect_err(|err| error!(?err, "list_jobs error"))?;
            cloud_job_ids.push(CloudJobId(summary.job_id.expect("job_id")));
        }
        // To get the tags, which will let us understand what the job is doing, we have to Describe all the jobs.
        let mut jobs = Vec::new();
        for chunk in cloud_job_ids.chunks(100) {
            let chunk2: Vec<&CloudJobId> = chunk.iter().collect();
            jobs.append(
                self.describe_jobs(&chunk2)
                    .await
                    .inspect_err(|err| error!(?err, "describe_job error"))?
                    .as_mut(),
            );
        }
        debug!("{} jobs listed", jobs.len());
        Ok(jobs)
    }

    async fn fetch_output(&self, job_name: &JobName, dest: &Path) -> Result<PathBuf> {
        let output_tarball_key = self.output_tarball_key(job_name);

        let output_tarball = self
            .s3_client
            .get_object()
            .bucket(&self.s3_bucket_name)
            .key(&output_tarball_key)
            .send()
            .await?;

        let local_output_tarball_path =
            dest.join(format!("mutants-remote-output-{job_name}.tar.zstd"));
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

impl From<&AwsJobStatus> for JobStatus {
    fn from(status: &AwsJobStatus) -> Self {
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
            _ => {
                warn!("Unknown AWS job status: {status:?}");
                JobStatus::Unknown
            }
        }
    }
}

impl From<Option<&AwsJobStatus>> for JobStatus {
    fn from(status: Option<&AwsJobStatus>) -> Self {
        match status {
            Some(status) => JobStatus::from(status),
            None => JobStatus::Unknown,
        }
    }
}

#[derive(Debug)]
struct AwsLogTail {
    #[allow(dead_code)] // just for debugging?
    pub log_stream_name: String,
    response_stream: EventReceiver<StartLiveTailResponseStream, StartLiveTailResponseStreamError>,
    warned_about_log_sampling: bool,
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
            warned_about_log_sampling: false,
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
                    if update
                        .session_metadata()
                        .is_some_and(|m| m.sampled && !self.warned_about_log_sampling)
                    {
                        warn!(?update, "Logs were sampled");
                        self.warned_about_log_sampling = true;
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
fn from_unix_millis(t: Option<i64>) -> Option<Timestamp> {
    match t {
        Some(0) | None => None,
        Some(t) => Timestamp::from_millisecond(t).ok(),
    }
}
