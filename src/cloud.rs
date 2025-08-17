//! Cloud abstraction for mutants-remote.
//!
//! Provides an aspirationally generic interface for cloud providers: the core features are to get and put files, launch jobs, and monitor the status of jobs.

use std::{
    fmt::{Display, Formatter},
    path::{Path, PathBuf},
};

use async_trait::async_trait;
use serde::Serialize;
use tracing::error;

use crate::job::{JobDescription, JobMetadata, JobName};
use crate::{Result, RunId, cloud::aws::AwsCloud, config::Config};

static OUTPUT_TARBALL_NAME: &str = "mutants.out.tar.zstd";

pub mod aws;

/// Abstraction of a cloud provider that can launch jobs, read their status,
/// fetch their logs or output tarball, etc.
#[async_trait]
pub trait Cloud {
    async fn upload_source_tarball(&self, run_id: &RunId, source_tarball: &Path) -> Result<()>;
    async fn submit_job(
        &self,
        job_name: &JobName,
        script: String,
        job_metadata: &JobMetadata,
    ) -> Result<CloudJobId>;
    async fn fetch_output(&self, job_name: &JobName) -> Result<PathBuf>;
    async fn tail_log(&self, job_description: &JobDescription) -> Result<Box<dyn LogTail>>;
    async fn describe_job(&self, job_id: &CloudJobId) -> Result<JobDescription>;

    /// List all jobs, including queued, running, and completed.
    async fn list_jobs(&self) -> Result<Vec<JobDescription>>;
}

/// Create a new cloud provider instance from the configuration.
pub async fn open_cloud(config: &Config) -> Result<Box<dyn Cloud>> {
    match AwsCloud::new(config.clone()).await {
        Ok(cloud) => Ok(Box::new(cloud)),
        Err(err) => {
            error!("Failed to initialize AWS cloud: {err}");
            Err(err)
        }
    }
}

/// The identifier for a job assigned by the cloud.
///
/// By contrast [`crate::JobName`] is the name chosen by us, before submitting the job.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct CloudJobId(String);

impl Display for CloudJobId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Abstract trait to tail logs from a single job running on a cloud.
#[async_trait]
pub trait LogTail {
    /// Fetch some more log events.
    ///
    /// Returns `Ok(None)` when the log stream has ended.
    async fn more_log_events(&mut self) -> Result<Option<Vec<String>>>;
}
