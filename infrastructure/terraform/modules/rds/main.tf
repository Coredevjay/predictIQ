variable "environment" {
  type = string
}

variable "vpc_id" {
  type = string
}

variable "private_subnet_ids" {
  type = list(string)
}

variable "db_name" {
  type = string
}

variable "db_username" {
  type      = string
  sensitive = true
}

variable "db_password" {
  type      = string
  sensitive = true
}

variable "db_instance_class" {
  type = string
}

variable "allocated_storage" {
  type = number
}

variable "backup_retention" {
  type    = number
  default = 7
}

variable "deletion_protection" {
  type    = bool
  default = false
}

variable "engine_version" {
  type        = string
  default     = "15"
  description = "PostgreSQL major version; minor versions are upgraded automatically."
}

variable "kms_key_id" {
  type        = string
  default     = null
  description = "ARN of a customer-managed KMS key for storage and Performance Insights encryption (null = AWS-managed key)."
}

variable "ecs_tasks_sg_id" {
  type        = string
  description = "Security group ID of the ECS tasks that are allowed to connect"
}

locals {
  common_tags = {
    Project   = "predictiq"
    Environment = var.environment
    Owner     = "infrastructure-team"
    ManagedBy = "terraform"
  }
}

resource "aws_db_subnet_group" "main" {
  name       = "predictiq-${var.environment}-db-subnet"
  subnet_ids = var.private_subnet_ids

  tags = merge(
    local.common_tags,
    {
      Name = "predictiq-${var.environment}-db-subnet-group"
    }
  )
}

resource "aws_security_group" "rds" {
  name   = "predictiq-${var.environment}-rds-sg"
  vpc_id = var.vpc_id

  # Inbound PostgreSQL from ECS tasks only
  ingress {
    from_port       = 5432
    to_port         = 5432
    protocol        = "tcp"
    security_groups = [var.ecs_tasks_sg_id]
  }

  tags = merge(
    local.common_tags,
    {
      Name = "predictiq-${var.environment}-rds-sg"
    }
  )
}

resource "aws_db_instance" "main" {
  identifier            = "predictiq-${var.environment}"
  engine                = "postgres"
  engine_version        = var.engine_version
  auto_minor_version_upgrade = true
  instance_class        = var.db_instance_class
  allocated_storage     = var.allocated_storage
  storage_encrypted     = true
  kms_key_id            = var.kms_key_id
  db_name               = var.db_name
  username              = var.db_username
  password              = var.db_password
  db_subnet_group_name  = aws_db_subnet_group.main.name
  vpc_security_group_ids = [aws_security_group.rds.id]
  
  backup_retention_period = var.backup_retention
  backup_window           = "03:00-04:00"
  maintenance_window      = "mon:04:00-mon:05:00"
  deletion_protection     = var.environment == "prod" ? true : var.deletion_protection

  multi_az               = var.environment == "prod" ? true : false
  publicly_accessible    = false
  skip_final_snapshot    = var.environment != "prod"
  final_snapshot_identifier = var.environment == "prod" ? "predictiq-${var.environment}-final-snapshot" : null

  performance_insights_enabled    = true
  performance_insights_kms_key_id = var.kms_key_id
  enabled_cloudwatch_logs_exports = ["postgresql", "upgrade"]

  tags = merge(
    local.common_tags,
    {
      Name = "predictiq-${var.environment}-db"
    }
  )

  lifecycle {
    prevent_destroy = true
  }
}

output "sg_id" {
  value = aws_security_group.rds.id
}

output "endpoint" {
  value     = aws_db_instance.main.endpoint
  sensitive = true
}
