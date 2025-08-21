// Copyright 2025 Martin Pool

//! A "run" is one execution of a set of mutations on a set of target.
//!
//! A run is implemented on a cloud as a set of jobs, each of which runs on one VM.

use std::{path::Path, str::FromStr};

use serde::Serialize;
use time::{OffsetDateTime, macros::format_description};

use crate::{
    error::{Error, Result},
    tags::{
        CLIENT_HOSTNAME_TAG, CLIENT_USERNAME_TAG, MUTANTS_REMOTE_VERSION_TAG, SOURCE_DIR_TAIL_TAG,
    },
};

/// Additional metadata attached to all the resources in one run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunMetadata {
    /// The tail of the source directory path.
    pub source_dir_tail: Option<String>,
    /// The client hostname.
    pub client_hostname: Option<String>,
    /// The local user name on the client.
    pub client_username: Option<String>,
    /// The version of mutants-remote that created this job.
    pub mutants_remote_version: Option<String>,
}

impl RunMetadata {
    pub fn new(source_dir: &Path) -> Self {
        RunMetadata {
            source_dir_tail: source_dir
                .file_name()
                .map(|f| f.to_string_lossy().into_owned()),
            client_hostname: hostname::get()
                .ok()
                .map(|h| h.to_string_lossy().into_owned()),
            client_username: Some(whoami::username()),
            mutants_remote_version: Some(crate::VERSION.to_string()),
        }
    }

    /// Translate the metadata to a series of string tags.
    pub fn to_tags(&self) -> Vec<(&'static str, String)> {
        let mut tags = Vec::with_capacity(4);
        if let Some(dir) = &self.source_dir_tail {
            tags.push((SOURCE_DIR_TAIL_TAG, dir.to_string()));
        }
        if let Some(host) = &self.client_hostname {
            tags.push((CLIENT_HOSTNAME_TAG, host.to_string()));
        }
        if let Some(user) = &self.client_username {
            tags.push((CLIENT_USERNAME_TAG, user.to_string()));
        }
        if let Some(version) = &self.mutants_remote_version {
            tags.push((MUTANTS_REMOTE_VERSION_TAG, version.to_string()));
        }
        tags
    }
}

/// Select one or all runs for some operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KillTarget {
    All,
    ById(Vec<RunId>),
}

/// Identifier assigned by us to a run.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize)]
pub struct RunId(String);

impl RunId {
    /// Generate a probably-unique ID for a run.
    pub fn from_clock() -> RunId {
        let now = OffsetDateTime::now_utc();
        let time_str = now
            .format(format_description!(
                "[year][month][day]-[hour][minute][second]"
            ))
            .unwrap();
        // Maybe it's quirky, but to make the strings easier to visually match,
        // we encode fractional seconds in hex.
        RunId(format!(
            "{time}-{suffix:04x}",
            time = time_str,
            suffix = now.microsecond() & 0xFFFF
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
