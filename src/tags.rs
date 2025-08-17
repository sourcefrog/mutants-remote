// Copyright 2025 Martin Pool

//! Tags attached to cloud resources.

/// The name of a tag identifying a run, attached to jobs and other resources created for the run.
///
/// The value of the tag is the run ID.
pub static RUN_ID_TAG: &str = "mutants-remote/run-id";

/// A tag on jobs holding the tail of the source directory path.
pub static SOURCE_DIR_TAIL_TAG: &str = "mutants-remote/source-dir-tail";
