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
    runtimePlatform = {
      cpuArchitecture       = "X86_64" # TODO: support ARM64 when the mutants image supports it
      operatingSystemFamily = "LINUX"
    }
    taskProperties = [
      {
        platformVersion  = "LATEST",
        executionRoleArn = "arn:aws:iam::${local.account_id}:role/${var.execution_role_name}"
        taskRoleArn      = "arn:aws:iam::${local.account_id}:role/${var.task_role_name}"
        networkConfiguration = {
          assignPublicIp = "ENABLED" # needed to reach the internet and the containers
        }
        containers = [
          {
            name    = "mutants-remote"
            command = ["cargo", "mutants", "--version"] # expected to be overridden per job
            image   = "${local.account_id}.dkr.ecr.us-west-2.amazonaws.com/github/sourcefrog/cargo-mutants:container"
            # image = "ghcr.io/sourcefrog/cargo-mutants:container"
            user = "mutants"
            resourceRequirements = [
              # See https://docs.aws.amazon.com/batch/latest/APIReference/API_ResourceRequirement.html
              {
                type  = "VCPU"
                value = "${var.vcpu}"
              },
              {
                type  = "MEMORY"
                value = "${var.memory}"
              }
            ]
            dependsOn   = []
            environment = []
            mountPoints = []
            secrets     = []
          },
        ]
        volumes = []
      }
    ]
  })
}

resource "aws_iam_role" "execution" {
  name        = var.execution_role_name
  description = "Execution role for mutants-remote ECS tasks, allowing them to fetch container images and write logs"
  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Action = "sts:AssumeRole"
        Effect = "Allow"
        Principal = {
          Service = "ecs-tasks.amazonaws.com"
        }
      }
    ]
  })
}

resource "aws_iam_role_policy_attachments_exclusive" "execution" {
  role_name   = aws_iam_role.execution.name
  policy_arns = ["arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"]
}

resource "aws_iam_role" "task" {
  name        = var.task_role_name
  description = "Task role for mutants-remote ECS tasks, allowing them to fetch container images and write logs"
  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Action = "sts:AssumeRole"
        Effect = "Allow"
        Principal = {
          Service = "ecs-tasks.amazonaws.com"
        }
      }
    ]
  })
}

resource "aws_iam_policy" "task" {
  name        = var.task_role_name
  description = "Task role for mutants-remote ECS tasks, allowing them to read input and output objects from S3"
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Action = [
          "s3:GetObject",
          "s3:PutObject",
          "s3:ListBucket",
        ]
        Effect   = "Allow"
        Resource = "*"
      }
    ]
  })
}

resource "aws_iam_role_policy_attachments_exclusive" "task" {
  role_name = aws_iam_role.task.name
  policy_arns = [
    aws_iam_policy.task.arn,
  ]
}
