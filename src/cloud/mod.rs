//! Cloud abstraction for mutants-remote.
//!
//! Provides an aspirationally generic interface for cloud providers: the core features are to get and put files, launch jobs, and monitor the status of jobs.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use thiserror::Error;

static SUITE_ID_TAG: &str = "mutants-remote-suite";
static OUTPUT_TARBALL_NAME: &str = "mutants.out.tar.zstd";

pub mod aws;

#[derive(Error, Debug)]
pub enum CloudError {
    #[error("Cloud provider error: {0}")]
    Provider(#[from] Box<dyn std::error::Error>),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Abstraction of a cloud provider that can launch jobs, monitor them, and fetch their output, etc.
#[async_trait]
pub trait Cloud {
    async fn upload_source_tarball(&self, source_tarball: &Path) -> Result<(), CloudError>;
    async fn submit_job(&self, script: String, job_name: String) -> Result<CloudJobId, CloudError>;
    async fn monitor_job(&self, job_id: &CloudJobId) -> Result<(), CloudError>;
    async fn fetch_output(&self, job_id: &CloudJobId) -> Result<PathBuf, CloudError>;
    async fn log_tail(&self, job_id: &CloudJobId) -> Result<Box<dyn LogTail>, CloudError>;
}

/// The identifier for a job assigned by the cloud.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CloudJobId(String);

/// Abstract trait to tail logs from a single job running on a cloud.
#[async_trait]
pub trait LogTail {
    /// Fetch some more log events.
    ///
    /// Returns `Ok(None)` when the log stream has ended.
    async fn more_log_events(&mut self) -> Result<Option<Vec<String>>, CloudError>;
}
