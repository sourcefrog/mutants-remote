data "aws_caller_identity" "current" {}

locals {
  account_id = data.aws_caller_identity.current.account_id
}

resource "aws_batch_job_definition" "mutants-amd64" {
  name = "mutants-amd64"
  type = "container"
  timeout {
    attempt_duration_seconds = 7200
  }
  platform_capabilities = ["FARGATE"]
  ecs_properties = jsonencode({
    taskProperties = [
      {
        executionRoleArn = "arn:aws:iam::${local.account_id}:role/mutants-batch-execution"
        taskRoleArn      = "arn:aws:iam::${local.account_id}:role/mutants-batch-execution" # TODO: Should they be different?
        networkConfiguration = {
          assignPublicIp = "ENABLED"
        }
        containers = [
          {
            name    = "mutants-remote"
            command = ["cargo", "mutants", "--version"] # expected to be overridden per job
            image   = "${local.account_id}.dkr.ecr.us-west-2.amazonaws.com/github/sourcefrog/cargo-mutants:container"
            # image = "ghcr.io/sourcefrog/cargo-mutants:container"
            user  = "mutants"
            resourceRequirements = [
              {
                type  = "VCPU"
                value = "2"
              },
              {
                type  = "MEMORY"
                value = "4096"
              }
            ]
          }
        ]
      }
    ]
  })
}
