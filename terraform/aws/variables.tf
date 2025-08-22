variable "vcpu" {
  type        = string # a string in the AWS API because you can specify precise fractions
  description = "The number of vCPUs in each batch worker instance. Valid values are 1, 2, 4, 8,16. Valid combinations with memory are listed in the AWS documentation."
  default     = 16
}

variable "memory" {
  type        = string
  description = "The amount of memory in GB for each batch worker instance. Valid combinations with vCPU are listed in the AWS documentation."
  default     = 32768
}
