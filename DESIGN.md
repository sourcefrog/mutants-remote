# mutants-remote design

## Overview

Testing a set of mutants in some tree, by running `mutants-remote run`, creates one `Run` object. When the Run does parallel sharded work, it's rendered into a set of one or more remote `Job` objects.

The major stages in testing mutants are:

1. Tar up the source tree, possibly with exclusions.
2. Upload the source to some staging location where it will be accessible to jobs.
3. Launch the appropriate number of sharded jobs.
4. Monitor the jobs until they complete.
5. Fetch and unpack the output files from all jobs.

The Run has a time-based `RunId` assigned by the client; the jobs also have a client-assigned `JobName` including the `RunId`. We also expect that the cloud will assign its own native ID for jobs, which we call a `CloudJobId`.

## AWS-Batch

AWS Batch has the advantage that there are no fixed costs: you only pay for compute time and storage.

AWS Batch stores input and output files in an S3 bucket. Logs are stored in CloudWatch logs.

Metadata for AWS Batch is stored in tags on the job resources, so that later clients can understand what jobs were previously run without retaining a separate database or history. In particular, a tag points from the job to its output file.

The bucket should be configured to expire objects some time after they're used. The Terraform in `terraform/aws` does this.

Because there are limits on how long the command for any job can be, we write a script into the S3 buckets. Each worker fetches and executes that script.

AWS Batch has a concept of 'matrix' jobs, but unfortunately you cannot create a matrix of size 1. To avoid a discontinuity in behavior between `--shards=1` and `>1` we always spawn individual jobs for each shard.
