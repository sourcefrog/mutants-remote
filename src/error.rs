use thiserror::Error;

use crate::cloud::CloudError;

// TODO: Also try `thistermination` to give specific error codes...
#[derive(Error, Debug)]
pub enum Error {
    #[error("Cloud error: {0}")]
    Cloud(#[from] CloudError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

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
