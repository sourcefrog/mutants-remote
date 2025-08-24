// Copyright 2025 Martin Pool

//! Spawn tasks in Docker containers.

#![allow(unused)] // until implemented

use std::path::{Path, PathBuf};
use std::str::FromStr;

use async_trait::async_trait;
use aws_sdk_batch::operation::describe_job_definitions;
use serde_json::Value;
use time::OffsetDateTime;
use tokio::fs::{create_dir, create_dir_all};
use tokio::process::Command;
use tracing::{debug, error, warn};

use crate::SOURCE_TARBALL_NAME;
use crate::cloud::{CONTAINER_USER, Cloud, CloudJobId, LogTail, OUTPUT_TARBALL_NAME};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::job::{JobDescription, JobName, JobStatus, central_command};
use crate::run::{KillTarget, RunArgs, RunId, RunMetadata};

static JOB_MOUNT: &str = "/job";

/// Docker cloud implementation.
///
/// On Docker, the job name is set to the job name using the `--name` argument,
/// so the CloudJobName is the same as the job name.
#[derive(Debug)]
pub struct Docker {
    /// Directory holding bind mounts to Docker for logs, output, etc.
    state_dir: PathBuf,
    config: Config,
}

impl Docker {
    pub fn new(config: Config) -> Result<Self> {
        let state_dir = dirs::data_local_dir()
            .expect("Failed to get data local directory")
            .join("mutants-remote")
            .join("docker");
        std::fs::create_dir_all(&state_dir).inspect_err(|err| {
            error!(
                "Failed to create state directory {} : {}",
                state_dir.display(),
                err
            )
        });
        Ok(Self { state_dir, config })
    }

    fn run_dir(&self, run_id: &RunId) -> PathBuf {
        self.state_dir.join(run_id.to_string())
    }

    /// Return the path of a local directory dedicated to a job.
    fn job_dir(&self, job_name: &JobName) -> PathBuf {
        self.state_dir
            .join(job_name.run_id.to_string())
            .join(job_name.to_string())
    }
}

#[async_trait]
impl Cloud for Docker {
    async fn submit(
        &self,
        run_id: &RunId,
        run_metadata: &RunMetadata,
        run_args: &RunArgs,
        source_tarball: &Path,
    ) -> Result<(JobName, CloudJobId)> {
        let job_name = JobName::new(run_id, 0);
        let job_dir = self.job_dir(&job_name);
        create_dir_all(&job_dir).await?;

        tokio::fs::copy(source_tarball, job_dir.join(SOURCE_TARBALL_NAME)).await?;

        let mut command = Command::new("docker");
        let shard_k = 0;
        let shard_n = 1;
        // TODO: Maybe use 'docker cp' to upload source and download results instead of bind mounts; that
        // would be a bit more similar to remote clouds.
        // TODO: Job metadata into labels
        let script = central_command(run_args, shard_k, shard_n);
        let wrapped_script = format!(
            "
            free -m && id && pwd && df -m &&
            cd &&
            pwd &&
            echo $HOME &&
            export CARGO_HOME=$HOME/.cargo &&
            mkdir work &&
            cd work &&
            tar xf {JOB_MOUNT}/{SOURCE_TARBALL_NAME} --zstd &&
            {script}
            tar cf {JOB_MOUNT}/mutants.out.tar.zstd mutants.out --zstd
            ",
        );
        debug!(?wrapped_script);
        std::fs::write(job_dir.join("script.sh"), wrapped_script)?;
        command
            .args(["container", "run"])
            .arg(format!("--name={job_name}"))
            .arg(format!("--user={CONTAINER_USER}"))
            .arg(format!(
                "--mount=type=bind,source={job_dir},target={JOB_MOUNT}",
                job_dir = job_dir.display()
            ))
            .arg(self.config.image_name_or_default())
            .args(["bash", "-ex"])
            .arg(format!("{JOB_MOUNT}/script.sh"));
        debug!(?command);
        let mut child = command.spawn().inspect_err(|err| {
            error!("Failed to spawn docker container: {}", err);
        })?;
        let result = child
            .wait()
            .await
            .inspect_err(|err| error!("Failed to wait for docker run command: {err}"))?;
        if !result.success() {
            error!("Docker run command failed with exit code {result:?}");
            return Err(Error::JobDidNotStart);
        }
        debug!(?job_name, "Started Docker container");
        Ok((job_name.clone(), CloudJobId::new(&job_name.to_string())))
    }

    async fn fetch_output(&self, job_name: &JobName, dest_dir: &Path) -> Result<PathBuf> {
        let output_path = self.job_dir(job_name).join(OUTPUT_TARBALL_NAME);
        let dest_path = dest_dir.join(OUTPUT_TARBALL_NAME);
        tokio::fs::copy(&output_path, &dest_path)
            .await
            .inspect_err(|err| {
                error!("Failed to copy output file: {}", err);
            })?;
        Ok(dest_path)
    }

    async fn tail_log(&self, job_description: &JobDescription) -> Result<Box<dyn LogTail>> {
        todo!("Docker::tail_log")
    }

    async fn describe_job(&self, job_id: &CloudJobId) -> Result<JobDescription> {
        let mut command = Command::new("docker");
        command
            .args(["ps", "--format=json", "-a"])
            .arg(format!("--filter=name={job_id}"));
        let output = command.output().await.inspect_err(|err| {
            error!("Failed to execute docker ps command: {}", err);
        })?;
        if !output.status.success() {
            error!(
                "Docker ps command failed with exit code {:?}",
                output.status
            );
            return Err(Error::Docker(output.status.code().unwrap()));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        debug!(?stdout, "Docker ps output");
        let ps: serde_json::Value = serde_json::from_str(&stdout).inspect_err(|err| {
            error!("Failed to parse docker ps output: {}", err);
        })?;
        debug!(?ps, "Parsed docker ps output");
        parse_docker_ps_output(&ps)
    }

    /// List all jobs, including queued, running, and completed.
    async fn list_jobs(&self, since: Option<OffsetDateTime>) -> Result<Vec<JobDescription>> {
        todo!("Docker::list_jobs")
    }

    /// Kill all jobs associated with a run.
    async fn kill(&self, kill_target: KillTarget) -> Result<()> {
        todo!()
    }
}

fn parse_docker_ps_output(ps: &serde_json::Value) -> Result<JobDescription> {
    let dict = ps
        .as_object()
        .ok_or(Error::Format("Invalid docker ps JSON format"))?;
    // It could have multiple names but we expect just one?
    let name = dict
        .get("Names")
        .and_then(Value::as_str)
        .ok_or(Error::Format("Invalid names in docker ps output"))?;
    let job_name = JobName::from_str(name)
        .map_err(|err| Error::Format("$Can't parse job name from docker output"))?;
    let docker_state = dict
        .get("State")
        .and_then(Value::as_str)
        .ok_or(Error::Format("Invalid state in docker ps output"))?;
    let status = match docker_state {
        "running" => JobStatus::Running,
        "exited" => JobStatus::Completed,
        _ => {
            warn!("JobStatus::Unknown, docker_state: {}", docker_state);
            JobStatus::Unknown
        }
    };
    Ok(JobDescription {
        cloud_job_id: CloudJobId(name.to_string()),
        status,
        status_reason: dict
            .get("Status")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        log_stream_name: None,
        raw_job_name: Some(name.to_string()),
        job_name: Some(job_name),
        created_at: None, // TODO : more fields
        started_at: None,
        stopped_at: None,
        cloud_tags: None,
        run_metadata: None,
    })
}
