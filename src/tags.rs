// Copyright 2025 Martin Pool

//! Tags attached to cloud resources.

/// The name of a tag identifying a run, attached to jobs and other resources created for the run.
///
/// The value of the tag is the run ID.
pub static RUN_ID_TAG: &str = "mutants-remote/run-id";

/// A tag on jobs holding the tail of the source directory path.
pub static SOURCE_DIR_TAIL_TAG: &str = "mutants-remote/source-dir-tail";

/// The client hostname.
pub static CLIENT_HOSTNAME_TAG: &str = "mutants-remote/client-hostname";

/// The local user name on the client.
pub static CLIENT_USERNAME_TAG: &str = "mutants-remote/client-username";

/// The version of mutants-remote that launched this job.
pub static MUTANTS_REMOTE_VERSION_TAG: &str = "mutants-remote/version";

/// The rfc9557 time that the job was created.
pub static RUN_START_TIME: &str = "mutants-remote/run-start-time";
