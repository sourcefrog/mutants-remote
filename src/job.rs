//! Cloud-independent job description and identifiers.

use std::str::FromStr;

use super::RunId;
use crate::cloud::CloudJobId;

/// Name assigned by us to a job within a run, including the run id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct JobName {
    pub run_id: RunId,
    // TODO: Maybe later an enum allowing for separate baseline and mutants jobs.
    pub shard_k: u32,
}

impl std::fmt::Display for JobName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-shard-{}", self.run_id, self.shard_k)
    }
}

impl FromStr for JobName {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split("-shard-").collect();
        if parts.len() != 2 {
            return Err("Job name doesn't look like {run_id}-shard-{shard_k}");
        }
        let run_id = RunId(parts[0].to_owned()); // For now assume anything's valid
        let shard_k = parts[1].parse().map_err(|_| "Invalid shard number")?;
        Ok(JobName { run_id, shard_k })
    }
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
    /// Parsed job name, it it can be parsed.
    pub job_name: Option<JobName>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_name_parsing() {
        let run_id = RunId("20250102030405-abcd".to_string());
        let shard_k = 13;
        let job_name = JobName {
            run_id: run_id.clone(),
            shard_k,
        };
        let job_name_str = job_name.to_string();
        assert_eq!(job_name_str, "20250102030405-abcd-shard-13");
        let parsed = JobName::from_str(&job_name_str).unwrap();
        assert_eq!(parsed.run_id, run_id);
        assert_eq!(job_name.shard_k, shard_k);
    }
}
