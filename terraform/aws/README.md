# mutants-remote AWS Terraform configuration

The Terraform module in this directory will configure some of the static resources required to run mutants-remote on AWS.

## Building infrastructure

Before applying this module, you must first create an AWS account and obtain the necessary credentials. It is  strongly recommended that you create a dedicated account for mutants-remote.

The Terraform module should be applied using admin-like credentials, since it configures IAM roles, groups, and policies.

With admin credentials in your environment, run `terraform init` and then `terraform apply` to create the infrastructure.

After making changes or fetching new code, run `terraform apply` again to update the infrastructure.

## IAM overview

Three different IAM principals are used in mutants-remote:

- Your admin user or role: used only to create the Terraform infrastructure.
- `mutants-batch-submission`: Your client machine uses this role to submit batch jobs: it has permission to write files to S3, create batch jobs, read logs, etc.
- batch execution role: This role is used by batch jobs to access the infrastructure.
- batch instance role: This role is accessible to the batch jobs, and is used to read and write S3 files.

## TODO

The following resources are not yet automated:

- IAM roles, groups, and policies
- Compute environment
- Batch queue
- S3 bucket
