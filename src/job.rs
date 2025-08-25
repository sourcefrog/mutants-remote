// Copyright 2025 Martin Pool

//! Cloud-independent job description and identifiers.

use std::{collections::HashMap, str::FromStr};

use jiff::Timestamp;
use serde::Serialize;
use shell_quote::QuoteRefExt;
use tracing::debug;

use super::RunId;
use crate::{
    cloud::CloudJobId,
    run::{RunArgs, RunMetadata},
};

/// Name assigned by us to a job within a run, including the run id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize)]
pub struct JobName {
    pub run_id: RunId,
    // TODO: Maybe later an enum allowing for separate baseline and mutants jobs.
    pub shard_k: u32,
}

impl JobName {
    pub fn new(run_id: &RunId, shard_k: u32) -> Self {
        JobName {
            run_id: run_id.clone(),
            shard_k,
        }
    }
}

impl std::fmt::Display for JobName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-shard-{:04}", self.run_id, self.shard_k)
    }
}

impl FromStr for JobName {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split("-shard-").collect();
        if parts.len() != 2 {
            return Err("Job name doesn't look like {run_id}-shard-{shard_k}");
        }
        let run_id = RunId::from_str(parts[0]).map_err(|_| "Invalid run ID")?;
        let shard_k = parts[1].parse().map_err(|_| "Invalid shard number")?;
        Ok(JobName { run_id, shard_k })
    }
}

/// Description of a job running or queued on a cloud.
///
/// This is parsed/interpreted from the raw description returned by the cloud.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JobDescription {
    /// The identifier for a job assigned by the cloud.
    pub cloud_job_id: CloudJobId,
    pub status: JobStatus,
    pub status_reason: Option<String>,
    /// Cloud-specific identifier of the log stream for this job, if it's known.
    // (This might later need to be generalized for other clouds?)
    pub log_stream_name: Option<String>,
    /// Raw job name as returned by the cloud.
    pub raw_job_name: Option<String>,
    /// Parsed job name, it it can be parsed.
    pub job_name: Option<JobName>,

    /// The time when the job was created (submitted)
    pub created_at: Option<Timestamp>,

    /// The time when the job started running
    pub started_at: Option<Timestamp>,

    pub stopped_at: Option<Timestamp>,
    pub cloud_tags: Option<HashMap<String, String>>,
    /// Structured metadata about the run this job is a part of.
    pub run_metadata: Option<RunMetadata>,
}

impl JobDescription {
    pub fn run_id(&self) -> Option<&RunId> {
        self.job_name.as_ref().map(|j| &j.run_id)
    }

    pub fn duration(&self) -> Option<std::time::Duration> {
        // TODO: Maybe for currently running or queued jobs, give the elapsed time since start?
        match (self.started_at, self.stopped_at) {
            // Sometimes in AWS batch some jobs that have failed have a start time but no end time.
            (Some(start), Some(stop)) => (stop - start).try_into().ok(), // only ok if >=0
            _ => None,
        }
    }

    /// If the job is still running or pending, return the duration since it started.
    pub fn elapsed(&self) -> Option<std::time::Duration> {
        if !self.status.is_dead() {
            self.created_at
                .and_then(|t| (Timestamp::now() - t).try_into().ok())
        } else {
            None
        }
    }
}

/// Describes the status of a job.
#[derive(Debug, Copy, derive_more::Display, Clone, PartialEq, Eq, Hash, Serialize)]
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

impl JobStatus {
    pub fn is_dead(&self) -> bool {
        match self {
            JobStatus::Completed | JobStatus::Failed => true,
            JobStatus::Submitted
            | JobStatus::Pending
            | JobStatus::Starting
            | JobStatus::Runnable
            | JobStatus::Running => false,
            JobStatus::Unknown => false,
        }
    }

    pub fn is_alive(&self) -> bool {
        *self != JobStatus::Unknown && !self.is_dead()
    }
}

/// Return the main command to be run.
///
/// This can be wrapped in cloud-specific commands.
pub fn central_command(run_args: &RunArgs, shard_k: u32, shard_n: u32) -> String {
    // TODO: Maybe Vec<String> instead?
    let script = format!(
        "cargo mutants {cargo_mutants_args} --shard {shard_k}/{shard_n} -vV || true",
        cargo_mutants_args = run_args
            .cargo_mutants_args
            .iter()
            .map(|a| a.quoted(shell_quote::Bash))
            .collect::<Vec<String>>()
            .join(" ")
    );
    debug!(?script);
    script
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_name_parsing() {
        let run_id = RunId::from_str("20250102030405-abcd").unwrap();
        let shard_k = 13;
        let job_name = JobName {
            run_id: run_id.clone(),
            shard_k,
        };
        let job_name_str = job_name.to_string();
        assert_eq!(job_name_str, "20250102030405-abcd-shard-0013");
        let parsed = JobName::from_str(&job_name_str).unwrap();
        assert_eq!(parsed.run_id, run_id);
        assert_eq!(job_name.shard_k, shard_k);
    }
}
