// Copyright 2025 Martin Pool

//! Tags attached to cloud resources.

// K8s says: https://kubernetes.io/docs/concepts/overview/working-with-objects/labels/
//
// Labels are key/value pairs. Valid label keys have two segments: an optional prefix and name,
// separated by a slash (/). The name segment is required and must be 63 characters or less,
// beginning and ending with an alphanumeric character ([a-z0-9A-Z]) with dashes (-), underscores
// (_), dots (.), and alphanumerics between. The prefix is optional. If specified, the prefix must
// be a DNS subdomain: a series of DNS labels separated by dots (.), not longer than 253 characters
// in total, followed by a slash (/).

/// The name of a tag identifying a run, attached to jobs and other resources created for the run.
///
/// The value of the tag is the run ID.
pub static RUN_ID_TAG: &str = "mutants.rs/run-id";

/// A tag on jobs holding the tail of the source directory path.
pub static SOURCE_DIR_TAIL_TAG: &str = "mutants.rs/source-dir-tail";

/// The client hostname.
pub static CLIENT_HOSTNAME_TAG: &str = "mutants.rs/client-hostname";

/// The local user name on the client.
pub static CLIENT_USERNAME_TAG: &str = "mutants.rs/client-username";

/// The version of mutants.rs that launched this job.
pub static MUTANTS_REMOTE_VERSION_TAG: &str = "mutants.rs/version";

/// The rfc9557 time that the job was created.
pub static RUN_START_TIME: &str = "mutants.rs/run-start-time";
