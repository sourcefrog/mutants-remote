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
