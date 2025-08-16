use std::path::PathBuf;

use thiserror::Error;

// TODO: Also try `thistermination` to give specific error codes...
#[derive(Error, Debug)]
pub enum Error {
    #[error("Cloud provider error: {0}")]
    Cloud(Box<dyn std::error::Error + Send + Sync>),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("IO error: {0} on {1}")]
    Path(std::io::Error, PathBuf),

    #[error("Invalid configuration: {0}")]
    Config(String),

    #[error("Tar failed: {0}")]
    Tar(String),
    // #[allow(dead_code)]
    // #[error("Job failed with status: {0}")]
    // JobFailed(String),

    // #[allow(dead_code)]
    // #[error("Job timed out")]
    // JobTimeout,

    // #[allow(dead_code)]
    // #[error("Invalid configuration: {0}")]
    // Config(String),
}

impl Error {
    pub fn on_path<P: Into<PathBuf>>(err: std::io::Error, path: P) -> Self {
        Self::Path(err, path.into())
    }
}
