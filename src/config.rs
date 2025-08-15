//! Configuration files for mutants-remote.

/// User-provided configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub aws_s3_bucket: String,
    pub aws_batch_job_queue: String,
    pub aws_batch_job_definition: String,
    pub aws_log_group_name: String,
}
