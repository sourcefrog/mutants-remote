variable "vcpu" {
  type        = string # a string in the AWS API because you can specify precise fractions
  description = "The number of vCPUs in each batch worker instance. Valid values are 1, 2, 4, 8,16. Valid combinations with memory are listed in the AWS documentation. This can be overridden in the mutants-remote configuration."
  default     = 16
}

variable "memory" {
  type        = string
  description = "The amount of memory in GB for each batch worker instance. Valid combinations with vCPU are listed in the AWS documentation. This can be overridden in the mutants-remote configuration."
  default     = 32768
}

variable "execution_role_name" {
  type        = string
  description = "The name of the execution role for the batch worker. This role is used to launch the batch worker instances."
  default     = "mutants-batch-execution"
}

variable "task_role_name" {
  type        = string
  description = "The name of the task role for the batch worker. This role is available to code running in the batch worker instances."
  default     = "mutants-batch-task"
}

variable "max_vcpus" {
  type        = number
  description = "The maximum number of vCPUs that can be simultaneously used by the batch worker instances."
  default     = 1024
}

variable "log_retention_days" {
  type        = number
  description = "The number of days to retain logs for the batch worker instances. Possible values are: 1, 3, 5, 7, 14, 30, 60, 90, 120, 150, 180, 365, 400, 545, 731, 1096, 1827, 2192, 2557, 2922, 3288, 3653, and 0. If you select 0, the events in the log group are always retained and never expire."
  default     = 90
}

variable "log_group_name" {
  type        = string
  description = "The name of the log group for the batch worker instances."
  default     = "mutants-remote"
}
