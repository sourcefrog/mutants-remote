// Copyright 2025 Martin Pool

//! Spawn tasks in Docker containers.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use async_trait::async_trait;
use jiff::Timestamp;
use serde_json::Value;
use tokio::fs::create_dir_all;
use tokio::process::Command;
use tracing::{debug, error, warn};

use crate::SOURCE_TARBALL_NAME;
use crate::cloud::{CONTAINER_USER, Cloud, CloudJobId, LogTail, OUTPUT_TARBALL_NAME};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::job::{JobDescription, JobName, JobStatus, central_command};
use crate::run::{KillTarget, RunArgs, RunId, RunLabels};
use crate::tags::RUN_ID_TAG;

static JOB_MOUNT: &str = "/job";

// TODO: Input and output from Docker is held in a `~/.local/share` directory, so that
// it outlasts the individual container run. These will accumulate over time, and
// garbage collection is not yet implemented.

/// Docker cloud implementation.
///
/// On Docker, the job name is set to the job name using the `--name` argument.
/// The CloudJobName is the container id returned when the container is created.
///
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
        })?;
        Ok(Self { state_dir, config })
    }

    #[allow(dead_code)]
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
        run_labels: &RunLabels,
        run_args: &RunArgs,
        source_tarball: &Path,
    ) -> Result<(JobName, CloudJobId)> {
        let job_name = JobName::new(run_id, 0);
        let job_dir = self.job_dir(&job_name);
        create_dir_all(&job_dir).await?;
        let container_id_path = job_dir.join("container_id");
        assert_eq!(
            run_args.shards, 1,
            "Multiple shards are not supported yet on Docker"
        );

        tokio::fs::copy(source_tarball, job_dir.join(SOURCE_TARBALL_NAME)).await?;

        let mut command = Command::new("docker");
        let shard_k = 0;
        let shard_n = 1;
        // TODO: Maybe use 'docker cp' to upload source and download results instead of bind mounts; that
        // would be a bit more similar to remote clouds.
        // TODO: Run --detach? That would be more like how it is done in remote clouds.
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
        // TODO: Add a tag for the start time, because Docker doesn't preserve it after the job exits.
        command
            .args(["container", "run"])
            .arg(format!("--name={job_name}"))
            .arg(format!("--user={CONTAINER_USER}"))
            .arg(format!("--label={RUN_ID_TAG}={run_id}"))
            .arg(format!("--cidfile={}", container_id_path.to_str().unwrap()))
            .arg(format!(
                "--mount=type=bind,source={job_dir},target={JOB_MOUNT}",
                job_dir = job_dir.display()
            ));
        for (key, value) in run_labels.to_tags() {
            command.arg(format!("--label={key}={value}"));
        }
        // Caution: image name and args must come after all the options to Docker
        command
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
        let container_id = std::fs::read(&container_id_path)
            .inspect_err(|err| error!("Failed to read container ID file: {}", err))?;
        let container_id = String::from_utf8(container_id)
            .map_err(|_| Error::Format("Container id is not UTF-8"))?
            .trim()
            .to_owned();
        if container_id.is_empty() {
            error!("Container ID is empty");
            return Err(Error::JobDidNotStart);
        }
        debug!(?job_name, ?container_id, "Started Docker container");
        Ok((job_name.clone(), CloudJobId::new(&container_id)))
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

    async fn tail_log(&self, _job_description: &JobDescription) -> Result<Box<dyn LogTail>> {
        todo!("Docker::tail_log")
    }

    async fn describe_job(&self, cloud_job_id: &CloudJobId) -> Result<JobDescription> {
        let mut results = container_ls(&format!("id={cloud_job_id}")).await?;
        if results.len() == 1 {
            Ok(results.remove(0))
        } else {
            Err(Error::Format(
                "Expected exactly one container to match job ID",
            ))
        }
    }

    /// List all jobs, including queued, running, and completed.
    async fn list_jobs(&self, _since: Option<Timestamp>) -> Result<Vec<JobDescription>> {
        // TODO: Filter by time over the result, since there doesn't seem to be a native Docker filter.
        container_ls(&format!("label={RUN_ID_TAG}")).await
    }

    /// Kill all jobs associated with a run.
    async fn kill(&self, kill_target: KillTarget) -> Result<()> {
        let all_jobs = self.list_jobs(None).await?;
        let mut command = Command::new("docker");
        command.arg("kill");
        all_jobs
            .iter()
            .filter(|job| job.status == JobStatus::Running)
            .filter(|job| match &kill_target {
                KillTarget::ByRunId(run_ids) => job.run_id().is_some_and(|r| run_ids.contains(r)),
                KillTarget::All => true,
            })
            .for_each(|job| {
                command.arg(job.cloud_job_id.to_string());
            });
        debug!(?command, "Killing jobs");
        let exit = command.spawn()?.wait().await?;
        if exit.success() {
            Ok(())
        } else {
            error!("Docker kill failed with exit code {:?}", exit.code());
            Err(Error::Docker(exit.code().unwrap_or(1)))
        }
    }
}

/// List containers with a Docker filter string.
async fn container_ls(filters: &str) -> Result<Vec<JobDescription>> {
    let mut command = Command::new("docker");
    command
        .args(["ps", "--format=json", "-a"])
        .arg(format!("--filter={filters}"));
    debug!(?command, "Run container_ls");
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
    parse_docker_ps_output(&stdout)
}

/// Parse the output of `docker ps` command.
///
/// It has one json dict per line, each of which maps to a job description.
fn parse_docker_ps_output(container_ls_json: &str) -> Result<Vec<JobDescription>> {
    let mut jobs = Vec::new();
    for line in container_ls_json.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let ps: Value = serde_json::from_str(line).inspect_err(|err| {
            error!("Failed to parse docker ps output: {}", err);
        })?;
        debug!(?ps, "Parsed docker ps output");
        let dict = ps
            .as_object()
            .ok_or(Error::Format("Invalid docker ps JSON format"))?;
        // It could have multiple names but we expect just one?
        let name = dict
            .get("Names")
            .and_then(Value::as_str)
            .ok_or(Error::Format("Invalid names in docker ps output"))?;
        let job_name = JobName::from_str(name)
            .map_err(|_| Error::Format("$Can't parse job name from docker output"))?;
        let docker_state = dict
            .get("State")
            .and_then(Value::as_str)
            .ok_or(Error::Format("Invalid state in docker ps output"))?;
        let container_id = dict
            .get("ID")
            .and_then(Value::as_str)
            .ok_or(Error::Format("Invalid id in docker ps output"))?;
        let status = match docker_state {
            "running" => JobStatus::Running,
            // TODO: Maybe check status and map to "failed"
            "exited" => JobStatus::Completed,
            _ => {
                warn!("JobStatus::Unknown, docker_state: {}", docker_state);
                JobStatus::Unknown
            }
        };
        let cloud_tags: Option<HashMap<String, String>> =
            dict.get("Labels").and_then(Value::as_str).map(|c| {
                c.split(',')
                    .flat_map(|s| {
                        s.split_once('=')
                            .map(|(k, v)| Some((k.to_string(), v.to_string())))
                    })
                    .flatten()
                    .collect::<HashMap<String, String>>()
            });
        let run_labels = cloud_tags.as_ref().map(RunLabels::from_tags);
        // "2025-08-24 10:29:23 -0700 PDT", but apparently only present for containers that are still alive.
        let docker_created_at = dict
            .get("Created")
            .and_then(|v| v.as_str())
            .and_then(|c| c.parse::<Timestamp>().ok());
        // Docker doesn't retain the start time for exited jobs so we use our own label if we have it.
        let started_at = run_labels
            .as_ref()
            .and_then(|m| m.run_start_time)
            .or(docker_created_at);
        jobs.push(JobDescription {
            cloud_job_id: CloudJobId(container_id.to_owned()),
            status,
            status_reason: dict
                .get("Status")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            log_stream_name: None, // not relevant, just use the container id
            raw_job_name: Some(name.to_string()),
            job_name: Some(job_name),
            started_at,
            created_at: started_at,
            stopped_at: None, // not reported exactly but we could get it from the string in the status
            cloud_tags,
            run_labels,
            output_tarball_url: None, // not implemented yet for Docker
        })
    }
    Ok(jobs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_docker_ps_output() {
        let json = r#"
            {"Command":"\"bash -ex /job/scrip…\"","CreatedAt":"2025-08-24 10:29:23 -0700 PDT","ID":"97d224f70f1e","Image":"ghcr.io/sourcefrog/cargo-mutants:container","Labels":"mutants-remote/run-id=20250824-172923-7c72,org.opencontainers.image.source=https://github.com/rust-lang/docker-rust","LocalVolumes":"0","Mounts":"/home/mbp/.loc…","Names":"20250824-172923-7c72-shard-0000","Networks":"bridge","Ports":"","RunningFor":"About a minute ago","Size":"0B","State":"exited","Status":"Exited (0) 54 seconds ago"}
        "#;
        let jobs = parse_docker_ps_output(json).unwrap();
        assert_eq!(jobs.len(), 1);
        let job0 = &jobs[0];
        assert_eq!(job0.cloud_job_id, CloudJobId::new("97d224f70f1e"));
        assert_eq!(job0.status, JobStatus::Completed);
        assert_eq!(
            job0.status_reason.as_ref().unwrap(),
            "Exited (0) 54 seconds ago"
        );
        assert_eq!(
            job0.raw_job_name.as_ref().unwrap(),
            "20250824-172923-7c72-shard-0000"
        );
        let mut tags = job0
            .cloud_tags
            .as_ref()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<Vec<(String, String)>>();
        tags.sort();
        assert_eq!(
            tags,
            &[
                (
                    "mutants-remote/run-id".to_string(),
                    "20250824-172923-7c72".to_string()
                ),
                (
                    "org.opencontainers.image.source".to_string(),
                    "https://github.com/rust-lang/docker-rust".to_string()
                ),
            ]
        );
        assert_eq!(
            job0.job_name.as_ref().unwrap(),
            &JobName::new(&RunId::from_str("20250824-172923-7c72").unwrap(), 0)
        );
    }
}
