// Copyright 2025 Martin Pool

//! Run cargo-mutants remotely on Kubernetes.

#![allow(dead_code, unused)]

use std::{
    fmt::Debug,
    path::{Path, PathBuf},
    str::FromStr,
};

use async_trait::async_trait;
use jiff::Timestamp;
use k8s_openapi::api::{
    batch::v1::{Job as K8sJob, JobSpec},
    core::v1::{Container, PodSpec, PodTemplateSpec},
};
use kube::{
    Client,
    api::{Api, DeleteParams, ListParams, ObjectMeta, PostParams},
    runtime::wait::{await_condition, conditions},
};
use tracing::{debug, error, info};

use crate::{
    cloud::{Cloud, CloudJobId, LogTail},
    config::Config,
    error::{Error, Result},
    job::{self, JobDescription, JobName, JobStatus},
    run::{KillTarget, RunArgs, RunId, RunLabels},
    tags::RUN_ID_TAG,
};

/// Kubernetes cloud implementation.
///
/// The JobName is used as both the `name` of the k8s Job and as the CloudJobId.
pub struct Kubernetes {
    client: kube::Client,
    jobs_api: Api<K8sJob>,
    config: Config,
}

impl Kubernetes {
    pub async fn new(config: Config) -> Result<Self> {
        let client = kube::Client::try_default()
            .await
            .inspect_err(|err| error!("Failed to create kube client: {err}"))?;
        let jobs_api = Api::<K8sJob>::namespaced(client.clone(), "default");
        Ok(Kubernetes {
            client,
            jobs_api,
            config,
        })
    }
}

#[async_trait]
impl Cloud for Kubernetes {
    async fn submit(
        &self,
        run_id: &RunId,
        run_labels: &RunLabels,
        run_args: &RunArgs,
        source_tarball: &Path,
    ) -> Result<(JobName, CloudJobId)> {
        // TODO: Copy the source tarball to some storage location accessible to the job
        // TODO: Spawn indexed jobs for the shards? Or just multiple jobs?
        let shard_k = 0;
        let job_name = JobName::new(run_id, shard_k);
        let name_string = job_name.to_string();
        let labels = Some(
            run_labels
                .to_labels()
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v))
                .collect(),
        );
        // TODO: Set command, image, etc
        let job = K8sJob {
            metadata: ObjectMeta {
                name: Some(name_string.clone()),
                labels,
                ..Default::default()
            },
            spec: Some(JobSpec {
                template: PodTemplateSpec {
                    spec: Some(PodSpec {
                        containers: vec![Container {
                            name: "main".to_owned(),
                            image: Some(self.config.image_name_or_default().to_owned()),
                            command: Some(
                                ["cargo", "mutants", "--version"]
                                    .iter()
                                    .map(|s| s.to_string())
                                    .collect(),
                            ),
                            // TODO: Should some be in args not command?
                            ..Default::default()
                        }],
                        restart_policy: Some("Never".to_owned()),
                        ..Default::default()
                    }),
                    metadata: None,
                },
                ..Default::default()
            }),
            status: None,
        };
        debug!(?job, "Creating job");
        let job = self
            .jobs_api
            .create(&PostParams::default(), &job)
            .await
            .inspect(|job| debug!(?job, "Job created"))
            .inspect_err(|err| error!("Failed to create job: {err}"))?;
        Ok((job_name.clone(), CloudJobId::new(&name_string)))
    }

    async fn fetch_output(&self, job_name: &JobName, dest_dir: &Path) -> Result<PathBuf> {
        todo!("Kubernetes::fetch_output")
    }
    async fn tail_log(&self, job_description: &JobDescription) -> Result<Box<dyn LogTail>> {
        todo!("Kubernetes::tail_log")
    }
    async fn describe_job(&self, job_id: &CloudJobId) -> Result<JobDescription> {
        todo!("Kubernetes::describe_job")
    }

    async fn list_jobs(&self, since: Option<Timestamp>) -> Result<Vec<JobDescription>> {
        // TODO: Labels into ListParams
        let params = ListParams {
            label_selector: Some(RUN_ID_TAG.to_string()),
            ..Default::default()
        };
        debug!(?params, "List jobs");
        let jobs = self.jobs_api.list(&params).await?;
        debug!("Listed jobs: {:?}", jobs);
        // TODO: To get a better status, perhaps we should really be looking at the individual pods within the job??
        let jobs = jobs
            .items
            .into_iter()
            .flat_map(|job| {
                JobDescription::try_from(job).inspect_err(|err| {
                    error!("Failed to parse job description: {}", err);
                })
            })
            .collect();
        Ok(jobs)
    }

    async fn kill(&self, kill_target: KillTarget) -> Result<()> {
        todo!()
    }
}

impl Debug for Kubernetes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Kubernetes").finish_non_exhaustive()
    }
}

impl From<kube::Error> for Error {
    fn from(err: kube::Error) -> Self {
        Error::Cloud(Box::new(err))
    }
}

impl TryFrom<K8sJob> for JobDescription {
    type Error = Error;

    fn try_from(value: K8sJob) -> std::result::Result<Self, Self::Error> {
        let name = value
            .metadata
            .name
            .ok_or(Error::Format("no name in k8s Job"))?;
        // TODO: More fields
        Ok(JobDescription {
            cloud_job_id: CloudJobId::new(&name),
            status: JobStatus::Unknown, // TODO
            status_reason: None,
            log_stream_name: None,
            raw_job_name: Some(name.clone()),
            job_name: JobName::from_str(&name).ok(),
            created_at: None,
            started_at: None,
            stopped_at: None,
            cloud_tags: None,
            run_labels: None,
        })
    }
}
