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
