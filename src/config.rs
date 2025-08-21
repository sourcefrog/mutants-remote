//! Configuration files for mutants-remote.

use std::{
    env::home_dir,
    path::{Path, PathBuf},
};

use schemars::JsonSchema;
use serde::Deserialize;
use tracing::debug;

use crate::{Error, Result};

/// Default configuration file name, relative to home.
static DEFAULT_CONFIG_FILE: &str = ".config/mutants-remote.toml";

/// Configuration for mutants-remote.
///
/// This is by default read from `~/.config/mutants-remote.toml`, or from the file specified by
/// `--config`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct Config {
    /// The name of the bucket to store artifacts, including input source tarballs and results.
    pub aws_s3_bucket: Option<String>,
    /// The name of the AWS Batch job queue to use.
    pub aws_batch_job_queue: Option<String>,
    /// The name of the AWS Batch job definition to use.
    pub aws_batch_job_definition: Option<String>,
    /// The AWS CloudWatch Logs group name to use.
    pub aws_log_group_name: Option<String>,

    /// Number of virtual CPU cores per job.
    pub vcpus: Option<u32>,
    /// Memory in megabytes per job.
    pub memory: Option<u32>,
}

impl Config {
    /// Load from a file, or load from the default location, or use builtin defaults.
    pub fn new(config_path: &Option<PathBuf>) -> Result<Self> {
        if let Some(config_path) = config_path {
            Self::from_file(config_path)
        } else {
            let default_path = home_dir()
                .expect("Couldn't determine home dir")
                .join(DEFAULT_CONFIG_FILE);
            if default_path.exists() {
                Self::from_file(&default_path)
            } else {
                debug!("No config file found, using defaults");
                Ok(Self::default())
            }
        }
    }

    pub(crate) fn from_file(config_path: &Path) -> Result<Self> {
        debug!(?config_path, "Loading config from file");
        let config_str = std::fs::read_to_string(config_path).map_err(|err| {
            Error::Config(format!(
                "Failed to load config file {}: {err}",
                config_path.display()
            ))
        })?;
        let config: Config = toml::from_str(&config_str).map_err(|err| {
            crate::Error::Config(format!(
                "Failed to parse config file {path}: {err}",
                path = config_path.display()
            ))
        })?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use assert_matches::assert_matches;
    use schemars::schema_for;
    use tempfile::NamedTempFile;

    use super::*;

    #[test]
    fn config_from_empty_file() {
        let config_tmp = NamedTempFile::new().unwrap();
        let config_path = config_tmp.path().to_path_buf();
        let config = Config::from_file(&config_path).unwrap();
        assert_eq!(config.aws_s3_bucket, None);
        assert_eq!(config.aws_batch_job_queue, None);
        assert_eq!(config.aws_batch_job_definition, None);
        assert_eq!(config.aws_log_group_name, None);
    }

    #[test]
    fn config_from_file() {
        let mut config_tmp = NamedTempFile::new().unwrap();
        config_tmp
            .write_all(
                br#"
                aws_s3_bucket = "my-bucket"
                aws_batch_job_queue = "my-queue"
                aws_batch_job_definition = "my-definition"
                aws_log_group_name = "my-log-group"
                "#,
            )
            .unwrap();
        let config_path = config_tmp.path().to_path_buf();
        let config = Config::from_file(&config_path).unwrap();
        assert_eq!(config.aws_s3_bucket, Some("my-bucket".to_string()));
        assert_eq!(config.aws_batch_job_queue, Some("my-queue".to_string()));
        assert_eq!(
            config.aws_batch_job_definition,
            Some("my-definition".to_string())
        );
        assert_eq!(config.aws_log_group_name, Some("my-log-group".to_string()));
    }

    #[test]
    fn config_from_file_with_errors() {
        let mut config_tmp = NamedTempFile::new().unwrap();
        config_tmp.write_all(b" garbage ").unwrap();
        let err = Config::from_file(config_tmp.path()).unwrap_err();
        assert_matches!(err, Error::Config(_));
        let msg = err.to_string();
        println!("{}", msg);
        assert!(msg.starts_with("Invalid configuration: Failed to parse config file"));
        assert!(msg.contains("garbage"));
        assert!(msg.contains(config_tmp.path().display().to_string().as_str()));
    }

    #[test]
    fn parse_example_config_from_source_tree() {
        let _config = Config::from_file(Path::new("example/mutants-remote.toml")).unwrap();
        // it's enough that it just parses
    }

    #[test]
    fn can_make_config_schema() {
        let schema = schema_for!(Config);
        let _schema_json = serde_json::to_string_pretty(&schema).unwrap();
    }
}
