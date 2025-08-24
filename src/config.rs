//! Configuration files for mutants-remote.

use std::{
    env::home_dir,
    path::{Path, PathBuf},
    str::FromStr,
};

use schemars::JsonSchema;
use serde::Deserialize;
use tracing::debug;

use crate::cloud::CloudProvider;
use crate::{Error, Result};

/// Default configuration file name, relative to home.
static DEFAULT_CONFIG_FILE: &str = ".config/mutants-remote.toml";

static DEFAULT_IMAGE_NAME: &str = "ghcr.io/sourcefrog/cargo-mutants:container";

/// Configuration for mutants-remote.
///
/// This is by default read from `~/.config/mutants-remote.toml`, or from the file specified by
/// `--config`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct Config {
    /// Which cloud provider to use.
    pub cloud_provider: Option<CloudProvider>,

    /// The Docker image to use for the remote environment.
    pub image_name: Option<String>,

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

    /// Exclude files matching these patterns from the source tarball.
    #[serde(default)]
    pub copy_exclude: Vec<String>,

    /// Additional args passed to remote cargo-mutants.
    #[serde(default)]
    pub cargo_mutants_args: Vec<String>,
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
        config_str.parse().map_err(|err| {
            Error::Config(format!(
                "Failed to parse config file {}: {err}",
                config_path.display()
            ))
        })
    }

    /// Return the image name or the builtin default.
    pub fn image_name_or_default(&self) -> &str {
        self.image_name.as_deref().unwrap_or(DEFAULT_IMAGE_NAME)
    }
}

impl FromStr for Config {
    type Err = toml::de::Error;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        toml::from_str(s)
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
        // No copy_exclude here: check that the default is empty.
        config_tmp
            .write_all(
                br#"
                aws_s3_bucket = "my-bucket"
                aws_batch_job_queue = "my-queue"
                aws_batch_job_definition = "my-definition"
                aws_log_group_name = "my-log-group"
                cargo_mutants_args = ['--cargo-arg=--config=linker="clang"', '--cargo-arg=--config=rustflags=["-C", "link-arg=--ld-path=wild"]']
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
        assert_eq!(config.copy_exclude, Vec::<String>::new());
        assert_eq!(
            config.cargo_mutants_args,
            [
                r#"--cargo-arg=--config=linker="clang""#,
                r#"--cargo-arg=--config=rustflags=["-C", "link-arg=--ld-path=wild"]"#
            ]
        );
        assert_eq!(config.cloud_provider, None);
    }

    #[test]
    fn cloud_provider_aws_batch() {
        let config = r#"
        cloud_provider = "AwsBatch"
        "#;

        let config = Config::from_str(config).unwrap();
        assert_eq!(config.cloud_provider, Some(CloudProvider::AwsBatch));
    }

    #[test]
    fn cloud_provider_docker() {
        let config = r#"
        cloud_provider = "Docker"
        "#;

        let config = Config::from_str(config).unwrap();
        assert_eq!(config.cloud_provider, Some(CloudProvider::Docker));
    }

    #[test]
    fn cloud_provider_unknown() {
        let config = r#"
        cloud_provider = "Unknown"
        "#;
        let err = Config::from_str(config).unwrap_err();
        println!("{err}");
        assert!(err.to_string().contains("unknown variant `Unknown`"));
    }

    #[test]
    fn config_copy_exclude() {
        let config = r#"
        copy_exclude = ["mutants.out*", ".git"]
        "#;

        let config = Config::from_str(config).unwrap();
        assert_eq!(config.copy_exclude, ["mutants.out*", ".git"]);
    }

    #[test]
    fn config_image_name() {
        let config = r#"
        image_name = "ghcr.io/sourcefrog/cargo-mutants:something"
        "#;

        let config = Config::from_str(config).unwrap();
        assert_eq!(
            config.image_name.unwrap(),
            "ghcr.io/sourcefrog/cargo-mutants:something"
        );
    }

    #[test]
    fn default_image_name() {
        let config = r#"
        "#;

        let config = Config::from_str(config).unwrap();
        assert_eq!(config.image_name, None);
        assert_eq!(
            config.image_name_or_default(),
            "ghcr.io/sourcefrog/cargo-mutants:container"
        );
    }
    #[test]
    fn config_from_file_with_errors() {
        let mut config_tmp = NamedTempFile::new().unwrap();
        config_tmp.write_all(b" garbage ").unwrap();
        let err = Config::from_file(config_tmp.path()).unwrap_err();
        assert_matches!(err, Error::Config(_));
        let msg = err.to_string();
        println!("{}", msg);
        assert!(msg.starts_with("Invalid configuration: "));
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
