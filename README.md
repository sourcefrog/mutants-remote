# mutants-remote: Run cargo-mutants in the cloud

An experimental tool to launch [cargo-mutants](https://github.com/sourcefrog/cargo-mutants) into AWS Batch jobs.
(There's room to later expand this to other remote environments such as other public clouds or k8s.)

⚠️ This is probably not usable by anyone else yet.

## Setup

Before running the tool you must manually create an AWS account with a bucket, batch queue, compute environment, and roles.

### AWS Credentials

AWS credentials are fetched from the [standard AWS credential provider chain](https://docs.aws.amazon.com/sdkref/latest/guide/standardized-credentials.html). Most likely, you will want to provide a credentials in the environment (`$AWS_SECRET_ACCESS_KEY` etc) or by pointing to a configured profile by setting `$AWS_PROFILE`.

## Security

This tool runs with permissions to read and write s3, read logs, and manipulate AWS batch jobs, within a single AWS account.

Assume that any write access to the account allows arbitrary code execution within it.

The AWS account used should contain no other resources and be accessible to only a single user.

As always, be careful not to commit or otherwise leak any credentials. Consider using something like `aws-vault` to keep local credentials safe.

This tool has not had second-party security review and may have security related bugs.
