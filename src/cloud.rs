//! Cloud abstraction for mutants-remote.
//!
//! Provides an aspirationally generic interface for cloud providers: the core features are to get and put files, launch jobs, and monitor the status of jobs.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use thiserror::Error;

use crate::{JobName, JobStatus, RunId};

/// The name of a tag identifying a run, attached to jobs and other resources created for the run.
///
/// The value of the tag is the run ID.
static RUN_ID_TAG: &str = "mutants-remote-run";
static OUTPUT_TARBALL_NAME: &str = "mutants.out.tar.zstd";

pub mod aws;

#[derive(Error, Debug)]
pub enum CloudError {
    #[error("Cloud provider error: {0}")]
    Provider(#[from] Box<dyn std::error::Error>),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Abstraction of a cloud provider that can launch jobs, read their status,
/// fetch their logs or output tarball, etc.
#[async_trait]
pub trait Cloud {
    async fn upload_source_tarball(
        &self,
        run_id: &RunId,
        source_tarball: &Path,
    ) -> Result<(), CloudError>;
    async fn submit_job(
        &self,
        job_name: &JobName,
        script: String,
    ) -> Result<CloudJobId, CloudError>;
    async fn fetch_output(&self, job_name: &JobName) -> Result<PathBuf, CloudError>;
    async fn tail_log(
        &self,
        job_description: &JobDescription,
    ) -> Result<Box<dyn LogTail>, CloudError>;
    async fn describe_job(&self, job_id: &CloudJobId) -> Result<JobDescription, CloudError>;
}

/// The identifier for a job assigned by the cloud.
///
/// By contrast [`crate::JobName`] is the name chosen by us, before submitting the job.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CloudJobId(String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct JobDescription {
    pub job_id: CloudJobId,
    pub status: JobStatus,
    /// Cloud-specific identifier of the log stream for this job, if it's known.
    // (This might later need to be generalized for other clouds?)
    pub log_stream_name: Option<String>,
    // TODO: The run id and shard number, extracted from tags on the job.
}

/// Abstract trait to tail logs from a single job running on a cloud.
#[async_trait]
pub trait LogTail {
    /// Fetch some more log events.
    ///
    /// Returns `Ok(None)` when the log stream has ended.
    async fn more_log_events(&mut self) -> Result<Option<Vec<String>>, CloudError>;
}
