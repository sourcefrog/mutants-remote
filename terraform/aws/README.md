# mutants-remote AWS Terraform configuration

The Terraform module in this directory will configure some of the static resources required to run mutants-remote on AWS.

Resources are created in the account and the default region of the credentials provided when running Terraform.

## Building infrastructure

Before applying this module, you must first create an AWS account and obtain the necessary credentials. It is  strongly recommended that you create a dedicated account for mutants-remote.

The Terraform module should be applied using admin-like credentials, since it configures IAM roles, groups, and policies.

With admin credentials in your environment, run `terraform init` and then `terraform apply` to create the infrastructure.

After making changes or fetching new code, run `terraform apply` again to update the infrastructure.

## Setting values

Some variables can be customized, by setting `-var` flags when running `terraform apply` or by [other means](https://developer.hashicorp.com/terraform/language/values/variables):

```
terraform apply -var="memory=8192"
```

## IAM overview

Three different IAM principals are used in mutants-remote:

- Your admin user or role: used only to create the Terraform infrastructure. This principal should not be accessible at runtime.
- `mutants-batch-submission`: Your client machine uses this role to submit batch jobs: it has permission to write files to S3, create batch jobs, read logs, etc.
- `mutants-batch-execution`: This role is assumed by the AWS Batch infrastructure and used to launch containers. It has permission to read containers and to rite logs.
- `mutants-batch-task`: This role is accessible to the batch jobs, and is used to read and write S3 files.

## Resource overview

- Bucket holding temporary input and output files, with auto-expiry of objects and a public access block
- Task and execution roles for AWS Batch
- AWS Batch job definition

## TODO

The following resources are not yet automated:

- Submission role: it's a bit unclear how to vend access to the submission role
- Compute environment
- Batch queue
