//! Cloud-independent job description and identifiers.

use super::RunId;
use crate::cloud::CloudJobId;

/// Name assigned by us to a job within a run, including the run id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct JobName {
    pub run_id: RunId,
    // TODO: Maybe later an enum allowing for separate baseline and mutants jobs.
    pub shard_k: u32,
}

/// Description of a job running or queued on a cloud.
///
/// This is parsed/interpreted from the raw description returned by the cloud.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct JobDescription {
    /// The identifier for a job assigned by the cloud.
    pub cloud_job_id: CloudJobId,
    pub status: JobStatus,
    /// Cloud-specific identifier of the log stream for this job, if it's known.
    // (This might later need to be generalized for other clouds?)
    pub log_stream_name: Option<String>,
    /// Raw job name as returned by the cloud.
    pub raw_job_name: Option<String>,
    // TODO: The run id and shard number, extracted from tags on the job.
}

/// Describes the status of a job.
#[derive(Debug, Copy, derive_more::Display, Clone, PartialEq, Eq, Hash)]
pub enum JobStatus {
    Submitted,
    Pending,
    Starting,
    Runnable,
    Running,
    Completed,
    Failed,
    Unknown,
}
