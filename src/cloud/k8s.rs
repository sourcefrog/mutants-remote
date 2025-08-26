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
use k8s_openapi::api::batch::v1::Job as K8sJob;
use kube::{
    Client,
    api::{Api, DeleteParams, ListParams, PostParams},
    runtime::wait::{await_condition, conditions},
};
use tracing::{debug, error, info};

use crate::{
    cloud::{Cloud, CloudJobId, LogTail},
    config::Config,
    error::{Error, Result},
    job::{JobDescription, JobName, JobStatus},
    run::{KillTarget, RunArgs, RunId, RunMetadata},
};

pub struct Kubernetes {
    client: kube::Client,
    jobs_api: Api<K8sJob>,
}

impl Kubernetes {
    pub async fn new(config: Config) -> Result<Self> {
        let client = kube::Client::try_default()
            .await
            .inspect_err(|err| error!("Failed to create kube client: {err}"))?;
        let jobs_api = Api::<K8sJob>::namespaced(client.clone(), "default");
        Ok(Kubernetes { client, jobs_api })
    }
}

#[async_trait]
impl Cloud for Kubernetes {
    async fn submit(
        &self,
        run_id: &RunId,
        run_metadata: &RunMetadata,
        run_args: &RunArgs,
        source_tarball: &Path,
    ) -> Result<(JobName, CloudJobId)> {
        todo!("Kubernetes::submit")
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
        let jobs = self.jobs_api.list(&ListParams::default()).await?;
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
            run_metadata: None,
        })
    }
}
