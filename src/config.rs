//! Configuration files for mutants-remote.

use std::path::Path;

use serde::Deserialize;

use crate::{Error, Result};

/// User-provided configuration.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Config {
    pub aws_s3_bucket: Option<String>,
    pub aws_batch_job_queue: Option<String>,
    pub aws_batch_job_definition: Option<String>,
    pub aws_log_group_name: Option<String>,
}

impl Config {
    pub(crate) fn from_file(config_path: &Path) -> Result<Self> {
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
}
