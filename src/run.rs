// Copyright 2025 Martin Pool

//! A "run" is one execution of a set of mutations on a set of target.
//!
//! A run is implemented on a cloud as a set of jobs, each of which runs on one VM.

use std::{collections::HashMap, path::Path, str::FromStr};

use jiff::Timestamp;
use serde::Serialize;

use crate::{
    error::{Error, Result},
    tags::{
        CLIENT_HOSTNAME_TAG, CLIENT_USERNAME_TAG, MUTANTS_REMOTE_VERSION_TAG, RUN_START_TIME,
        SOURCE_DIR_TAIL_TAG,
    },
};

/// Additional arguments for the run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunArgs {
    /// Extra arguments to pass to `cargo mutants`.
    pub cargo_mutants_args: Vec<String>,

    /// Number of parallel jobs to run.
    pub shards: usize,
}

/// Additional metadata attached to all the resources in one run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunLabels {
    /// The tail of the source directory path.
    pub source_dir_tail: Option<String>,
    /// The client hostname.
    pub client_hostname: Option<String>,
    /// The local user name on the client.
    pub client_username: Option<String>,
    /// The version of mutants-remote that created this job.
    pub mutants_remote_version: Option<String>,
    /// The start time of the run.
    pub run_start_time: Option<Timestamp>,
}

impl RunLabels {
    pub fn new(source_dir: &Path) -> Self {
        RunLabels {
            source_dir_tail: source_dir
                .file_name()
                .map(|f| f.to_string_lossy().into_owned()),
            client_hostname: hostname::get()
                .ok()
                .map(|h| h.to_string_lossy().into_owned()),
            client_username: Some(whoami::username()),
            mutants_remote_version: Some(crate::VERSION.to_string()),
            run_start_time: Some(Timestamp::now()),
        }
    }

    /// Translate metadata to "labels".
    ///
    /// Labels in k8s are constrained in what characters can occur in the values,
    /// are searchable, and are intended to be used only for identification and
    /// filtering.
    pub fn to_labels(&self) -> Vec<(&'static str, String)> {
        let mut labels = Vec::with_capacity(5);
        if let Some(dir) = &self.source_dir_tail {
            labels.push((SOURCE_DIR_TAIL_TAG, dir.to_string()));
        }
        if let Some(host) = &self.client_hostname {
            labels.push((CLIENT_HOSTNAME_TAG, host.to_string()));
        }
        if let Some(user) = &self.client_username {
            labels.push((CLIENT_USERNAME_TAG, user.to_string()));
        }
        if let Some(version) = &self.mutants_remote_version {
            labels.push((MUTANTS_REMOTE_VERSION_TAG, version.to_string()));
        }
        labels
    }

    /// Translate the metadata to a series of string tags.
    pub fn to_tags(&self) -> Vec<(&'static str, String)> {
        let mut tags = self.to_labels();
        if let Some(time) = &self.run_start_time {
            tags.push((RUN_START_TIME, time.to_string()));
        }
        tags
    }

    pub fn from_tags(tags: &HashMap<String, String>) -> Self {
        let source_dir_tail = tags.get(SOURCE_DIR_TAIL_TAG).cloned();
        let client_hostname = tags.get(CLIENT_HOSTNAME_TAG).cloned();
        let client_username = tags.get(CLIENT_USERNAME_TAG).cloned();
        let mutants_remote_version = tags.get(MUTANTS_REMOTE_VERSION_TAG).cloned();
        RunLabels {
            source_dir_tail,
            client_hostname,
            client_username,
            mutants_remote_version,
            run_start_time: tags.get(RUN_START_TIME).and_then(|s| s.parse().ok()),
        }
    }
}

/// Select one or all runs for some operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KillTarget {
    All,
    ByRunId(Vec<RunId>),
}

/// Identifier assigned by us to a run.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize)]
pub struct RunId(String);

impl RunId {
    /// Generate a probably-unique ID for a run.
    pub fn from_clock() -> RunId {
        let now = Timestamp::now();
        let time_str = now.strftime("%Y%m%d-%H%M%S").to_string();
        // Maybe it's quirky, but to make the strings easier to visually match,
        // we encode fractional seconds in hex.
        RunId(format!(
            "{time}-{suffix:05x}",
            time = time_str,
            suffix = now.subsec_microsecond()
        ))
    }
}

impl FromStr for RunId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        if s.is_empty() {
            Err(Error::InvalidRunId("Run ID cannot be empty".to_string()))
        } else {
            Ok(RunId(s.to_string()))
        }
    }
}

impl std::fmt::Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use regex::bytes::Regex;

    use super::*;

    #[test]
    fn roundtrip_run_id() {
        let id1 = RunId::from_clock();
        let id = &id1.0;
        let pattern = Regex::new(r"^20\d{6}-\d{6}-[a-f0-9]{5}$").unwrap();
        assert!(
            pattern.is_match(id.as_bytes()),
            "Run ID does not match pattern: {id:?}"
        );

        let id2 = RunId::from_str(id).unwrap();
        assert_eq!(id1, id2);

        assert_eq!(&format!("{id1}"), id);
    }

    #[test]
    fn roundtrip_run_labels_through_tags() {
        let metadata = RunLabels::new(Path::new("foo"));
        let tags: HashMap<String, String> = metadata
            .to_tags()
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        println!("{tags:?}");
        let metadata2 = RunLabels::from_tags(&tags);
        assert_eq!(metadata, metadata2);
    }
}
