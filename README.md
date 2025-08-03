# Run cargo-mutants on AWS Batch

An experimental tool to launch cargo-mutants into AWS Batch jobs.

To see any output you must set `RUST_LOG=info` before running the script.

## Setup

Before running the script:
 - Create an accounts, setup base resources
 - `aws sso login`
 - `aws configure`, set the profile for the account
 - `./mutants-batch.sh`

## One-time setup

* Create an account with a bucket, batch queue, compute environment, and roles

## Making images

(I could do this once on GitHub, no need for users to do it unless they want to change the image.)

(This can also run on GitHub except it's very slow for Arm builds.)

- Make an ECR repository
- Make a CodeBuild project


## TODO

- [ ] Docs/scripts/terraform to configure the static resources
- [ ] Build a Docker image instead of installing things at runtime?
