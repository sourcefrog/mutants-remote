# mutants-remote: Run cargo-mutants in the cloud

An experimental tool to launch [cargo-mutants](https://github.com/sourcefrog/cargo-mutants) into AWS Batch jobs.
(There's room to later expand this to other remote environments such as other public clouds or k8s.)

⚠️ This is probably not usable by anyone else yet.

## Setup

Before running the tool you must manually create an AWS account with a bucket, batch queue, compute environment, and roles. This is partially automated by the Terraform module in `terraform/aws`.

### AWS Credentials

AWS credentials are fetched from the [standard AWS credential provider chain](https://docs.aws.amazon.com/sdkref/latest/guide/standardized-credentials.html).

Most likely, you will want to configure an `awscli` profile and point to it by setting `$AWS_PROFILE`, or provide a credentials in the environment (`$AWS_SECRET_ACCESS_KEY` etc).

## Security

This tool runs with permissions to read and write s3, read logs, and manipulate AWS batch jobs, within a single AWS account.

Assume that any write access to the account allows arbitrary code execution within it.

The AWS account used should contain no other resources and be accessible to only a single user.

As always, be careful not to commit any credentials into git, or leak them in other ways. Consider using time-limited session credentials, e.g. through `aws configure sso`.

This tool has not had second-party security review and may have security related bugs.

## Usage

The given source directory is packaged into a tarball and uploaded to S3. By default, everything in the directory is included, but you can exclude files and directories using the `-e` flag. Probably you will want to exclude `target`, `mutants.out*` and `.git`.

### Configuration

mutants-remote reads a configuration file from `~/.config/mutants-remote.toml` or the path given by `--config`.

See `examples/config.toml` for an example configuration.

## Examples

You can pass arguments to the remote `cargo-mutants` after `--`, for example:

   mutants-remote run -d ~/src/conserve \
      -e .git \
      -e mutants.out\* \
      -e .jj \
      -e target \
       --no-default-features \
       -f archive.rs \
       --cargo-arg=--config='linker="clang"' \
       --cargo-arg=--config=rustflags='["-C", "link-arg=--ld-path=wild"]'
