variable "environment" {
  type = string
}

variable "vpc_id" {
  type = string
}

variable "private_subnet_ids" {
  type = list(string)
}

variable "public_subnet_ids" {
  type = list(string)
}

variable "api_image_uri" {
  type = string
}

variable "api_container_port" {
  type = number
}

variable "api_desired_count" {
  type = number
}

variable "api_cpu" {
  type = number
}

variable "api_memory" {
  type = number
}

# Redis connection components. The REDIS_URL is assembled by the application at
# runtime, so the credential-bearing URL never appears in Terraform state.
variable "redis_host" {
  type = string
}

variable "redis_port" {
  type    = number
  default = 6379
}

variable "redis_auth_token" {
  type      = string
  sensitive = true
}

variable "secret_rotation_lambda_arn" {
  type        = string
  default     = null
  description = "ARN of a rotation Lambda for DB/Redis secrets. When set, 30-day automatic rotation is enabled."
}

variable "log_retention_days" {
  type        = number
  default     = null
  description = "CloudWatch log retention. Defaults to 365 in prod and 7 elsewhere."
}

# Individual database credential components.
# Each is stored as its own Secrets Manager secret so they can be rotated
# independently, and the assembled connection string (containing the password)
# is never stored anywhere in plaintext.
variable "db_host" {
  type        = string
  description = "PostgreSQL hostname, e.g. rds-prod.cluster-xxxx.eu-west-1.rds.amazonaws.com"
}

variable "db_port" {
  type        = number
  default     = 5432
  description = "PostgreSQL port (default: 5432)"
}

variable "db_name" {
  type        = string
  description = "PostgreSQL database name, e.g. predictiq"
}

variable "db_user" {
  type        = string
  description = "PostgreSQL application user, e.g. predictiq_api"
}

variable "db_password" {
  type        = string
  sensitive   = true
  description = "PostgreSQL password for db_user. Store in your CI/CD secret store."
}

# SendGrid API key stored in Secrets Manager.
# Set this to the raw API key value; Terraform will create the secret version.
variable "sendgrid_api_key" {
  type      = string
  sensitive = true
}

# ISO-8601 date (YYYY-MM-DD) recording when SENDGRID_API_KEY was last rotated.
# Stored as Secrets Manager metadata so ops tooling and the startup check can
# read it without touching the key itself.
variable "sendgrid_key_rotated_at" {
  type        = string
  description = "Date the SendGrid API key was last rotated (YYYY-MM-DD, e.g. 2026-06-30)"
}

variable "acm_certificate_arn" {
  type        = string
  description = "ARN of the ACM certificate for HTTPS termination on the ALB."
}

variable "hmac_key" {
  description = "HMAC secret key used to sign API payloads"
  type        = string
  sensitive   = true
}

variable "api_signing_key" {
  description = "Private key used to sign API responses"
  type        = string
  sensitive   = true
}

variable "app_env" {
  type        = string
  description = "Value for APP_ENV/RUST_ENV; one of development, staging, production"
  validation {
    condition     = contains(["development", "staging", "production"], var.app_env)
    error_message = "app_env must be one of: development, staging, production."
  }
}

variable "base_url" {
  type        = string
  description = "Public base URL injected as BASE_URL"
}

variable "cors_allowed_origins" {
  type        = string
  description = "Comma-separated origins injected as CORS_ALLOWED_ORIGINS"
}

variable "from_email" {
  type        = string
  description = "Sender address injected as FROM_EMAIL"
}

variable "blockchain_rpc_url" {
  type        = string
  description = "Soroban RPC URL injected as BLOCKCHAIN_RPC_URL"
}

variable "predictiq_contract_id" {
  type        = string
  description = "Contract ID injected as PREDICTIQ_CONTRACT_ID"
}

variable "api_keys" {
  type        = string
  sensitive   = true
  description = "Comma-separated API keys injected as API_KEYS"
}

variable "unsubscribe_signing_secret" {
  type      = string
  sensitive = true
}

variable "sendgrid_webhook_secret" {
  type      = string
  sensitive = true
}

variable "email_idempotency_secret" {
  type      = string
  sensitive = true
}

variable "otlp_endpoint" {
  type        = string
  default     = ""
  description = "OTLP collector endpoint injected as OTLP_ENDPOINT (empty disables export)"
}

variable "ecs_tasks_sg_id" {
  type        = string
  description = "Security group ID of the ECS tasks (managed at root level)"
}

locals {
  extra_secrets = {
    api_keys                   = { env = "API_KEYS", value = var.api_keys }
    unsubscribe_signing_secret = { env = "UNSUBSCRIBE_SIGNING_SECRET", value = var.unsubscribe_signing_secret }
    sendgrid_webhook_secret    = { env = "SENDGRID_WEBHOOK_SECRET", value = var.sendgrid_webhook_secret }
    email_idempotency_secret   = { env = "EMAIL_IDEMPOTENCY_SECRET", value = var.email_idempotency_secret }
  }

  common_tags = {
    Project     = "predictiq"
    Environment = var.environment
    Owner       = "infrastructure-team"
    ManagedBy   = "terraform"
  }
}

# ── ECS Cluster ────────────────────────────────────────────────────────────────

resource "aws_ecs_cluster" "main" {
  name = "predictiq-${var.environment}"

  setting {
    name  = "containerInsights"
    value = "enabled"
  }

  tags = merge(local.common_tags, {
    Name = "predictiq-${var.environment}-cluster"
  })
}

# Per-environment customer-managed key for secrets and logs.
resource "aws_kms_key" "main" {
  description             = "predictiq-${var.environment} secrets and log encryption"
  enable_key_rotation     = true
  deletion_window_in_days = 30

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid       = "EnableRootAccount"
        Effect    = "Allow"
        Principal = { AWS = "arn:aws:iam::${data.aws_caller_identity.current.account_id}:root" }
        Action    = "kms:*"
        Resource  = "*"
      },
      {
        Sid       = "AllowCloudWatchLogs"
        Effect    = "Allow"
        Principal = { Service = "logs.${data.aws_region.current.name}.amazonaws.com" }
        Action    = ["kms:Encrypt*", "kms:Decrypt*", "kms:ReEncrypt*", "kms:GenerateDataKey*", "kms:Describe*"]
        Resource  = "*"
      }
    ]
  })

  tags = merge(local.common_tags, {
    Name = "predictiq-${var.environment}-kms"
  })
}

resource "aws_kms_alias" "main" {
  name          = "alias/predictiq-${var.environment}"
  target_key_id = aws_kms_key.main.key_id
}

resource "aws_cloudwatch_log_group" "ecs" {
  name              = "/ecs/predictiq-${var.environment}"
  retention_in_days = coalesce(var.log_retention_days, var.environment == "prod" ? 365 : 7)
  kms_key_id        = aws_kms_key.main.arn

  tags = merge(local.common_tags, {
    Name = "predictiq-${var.environment}-logs"
  })
}

# ── ECS Task Definition ────────────────────────────────────────────────────────

resource "aws_ecs_task_definition" "api" {
  family                   = "predictiq-${var.environment}-api"
  network_mode             = "awsvpc"
  requires_compatibilities = ["FARGATE"]
  cpu                      = var.api_cpu
  memory                   = var.api_memory
  execution_role_arn       = aws_iam_role.ecs_task_execution_role.arn
  task_role_arn            = aws_iam_role.ecs_task_role.arn

  container_definitions = jsonencode([
    {
      name      = "api"
      image     = var.api_image_uri
      essential = true
      portMappings = [
        {
          containerPort = var.api_container_port
          hostPort      = var.api_container_port
          protocol      = "tcp"
        }
      ]
      environment = concat([
        {
          name  = "ENVIRONMENT"
          value = var.environment
        },
        { name = "APP_ENV", value = var.app_env },
        { name = "RUST_ENV", value = var.app_env },
        { name = "BASE_URL", value = var.base_url },
        { name = "CORS_ALLOWED_ORIGINS", value = var.cors_allowed_origins },
        { name = "FROM_EMAIL", value = var.from_email },
        { name = "BLOCKCHAIN_RPC_URL", value = var.blockchain_rpc_url },
        { name = "PREDICTIQ_CONTRACT_ID", value = var.predictiq_contract_id }
        ], var.otlp_endpoint == "" ? [] : [
        {
          name  = "OTLP_ENDPOINT"
          value = var.otlp_endpoint
        }
      ])
      # All secrets are injected from AWS Secrets Manager — no plaintext
      # environment variables for sensitive values.
      secrets = concat([
        {
          name      = "DB_HOST"
          valueFrom = aws_secretsmanager_secret.db_host.arn
        },
        {
          name      = "DB_PORT"
          valueFrom = aws_secretsmanager_secret.db_port.arn
        },
        {
          name      = "DB_NAME"
          valueFrom = aws_secretsmanager_secret.db_name.arn
        },
        {
          name      = "DB_USER"
          valueFrom = aws_secretsmanager_secret.db_user.arn
        },
        {
          name      = "DB_PASSWORD"
          valueFrom = aws_secretsmanager_secret.db_password.arn
        },
        {
          name      = "REDIS_HOST"
          valueFrom = aws_secretsmanager_secret.redis_host.arn
        },
        {
          name      = "REDIS_PORT"
          valueFrom = aws_secretsmanager_secret.redis_port.arn
        },
        {
          name      = "REDIS_AUTH_TOKEN"
          valueFrom = aws_secretsmanager_secret.redis_auth_token.arn
        },
        {
          name      = "HMAC_KEY"
          valueFrom = aws_secretsmanager_secret.hmac_key.arn
        },
        {
          name      = "SENDGRID_API_KEY"
          valueFrom = aws_secretsmanager_secret.sendgrid_api_key.arn
        },
        {
          name      = "SENDGRID_KEY_ROTATED_AT"
          valueFrom = aws_secretsmanager_secret.sendgrid_key_rotated_at.arn
        },
        {
          # The API reads this value as REQUEST_SIGNING_SECRET (no reader exists for API_SIGNING_KEY).
          name      = "REQUEST_SIGNING_SECRET"
          valueFrom = aws_secretsmanager_secret.api_signing_key.arn
        }
      ], [
        for k, v in local.extra_secrets : {
          name      = v.env
          valueFrom = aws_secretsmanager_secret.extra[k].arn
        }
      ])
      logConfiguration = {
        logDriver = "awslogs"
        options = {
          "awslogs-group"         = aws_cloudwatch_log_group.ecs.name
          "awslogs-region"        = data.aws_region.current.name
          "awslogs-stream-prefix" = "ecs"
        }
      }
    }
  ])

  tags = merge(local.common_tags, {
    Name = "predictiq-${var.environment}-api-task"
  })
}

# ── Networking ─────────────────────────────────────────────────────────────────

resource "aws_security_group" "alb" {
  name   = "predictiq-${var.environment}-alb-sg"
  vpc_id = var.vpc_id

  # Allow inbound HTTP/HTTPS from the public internet
  ingress {
    from_port   = 80
    to_port     = 80
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  ingress {
    from_port   = 443
    to_port     = 443
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  # Restrict egress to the container port on ECS tasks only
  egress {
    from_port       = var.api_container_port
    to_port         = var.api_container_port
    protocol        = "tcp"
    security_groups = [var.ecs_tasks_sg_id]
  }

  tags = merge(local.common_tags, {
    Name = "predictiq-${var.environment}-alb-sg"
  })
}

# Allow inbound from the ALB on the container port — added as a rule on the
# externally-managed ecs_tasks SG to avoid a circular module dependency.
resource "aws_security_group_rule" "ecs_tasks_ingress_alb" {
  type                     = "ingress"
  from_port                = var.api_container_port
  to_port                  = var.api_container_port
  protocol                 = "tcp"
  security_group_id        = var.ecs_tasks_sg_id
  source_security_group_id = aws_security_group.alb.id
}

resource "aws_lb" "main" {
  name               = "predictiq-${var.environment}-alb"
  internal           = false
  load_balancer_type = "application"
  security_groups    = [aws_security_group.alb.id]
  subnets            = var.public_subnet_ids

  tags = merge(local.common_tags, {
    Name = "predictiq-${var.environment}-alb"
  })
}

resource "aws_lb_target_group" "api" {
  name        = "predictiq-${var.environment}-api"
  port        = var.api_container_port
  protocol    = "HTTP"
  vpc_id      = var.vpc_id
  target_type = "ip"

  health_check {
    healthy_threshold   = 2
    unhealthy_threshold = 3
    timeout             = 5
    interval            = 60
    path                = "/health/ready"
    matcher             = "200"
  }

  tags = merge(local.common_tags, {
    Name = "predictiq-${var.environment}-api-tg"
  })
}

resource "aws_lb_listener" "http" {
  load_balancer_arn = aws_lb.main.arn
  port              = 80
  protocol          = "HTTP"

  # Permanently redirect all plain-HTTP traffic to HTTPS (issue #889).
  # The ALB is the TLS termination point; the API service receives only
  # decrypted traffic from the ALB via the target group on port 80.
  default_action {
    type = "redirect"

    redirect {
      port        = "443"
      protocol    = "HTTPS"
      status_code = "HTTP_301"
    }
  }
}

resource "aws_lb_listener" "https" {
  load_balancer_arn = aws_lb.main.arn
  port              = 443
  protocol          = "HTTPS"
  ssl_policy        = "ELBSecurityPolicy-TLS13-1-2-2021-06"
  certificate_arn   = var.acm_certificate_arn

  default_action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.api.arn
  }
}

# ── ECS Service ────────────────────────────────────────────────────────────────

resource "aws_ecs_service" "api" {
  name            = "predictiq-${var.environment}-api"
  cluster         = aws_ecs_cluster.main.id
  task_definition = aws_ecs_task_definition.api.arn
  desired_count   = var.api_desired_count
  launch_type     = "FARGATE"

  # Zero-downtime rolling deploy: always keep 100 % capacity during updates,
  # allow up to 200 % so new tasks start before old ones are drained.
  deployment_minimum_healthy_percent = 100
  deployment_maximum_percent         = 200

  deployment_circuit_breaker {
    enable   = true
    rollback = true
  }

  network_configuration {
    subnets          = var.private_subnet_ids
    security_groups  = [var.ecs_tasks_sg_id]
    assign_public_ip = false
  }

  load_balancer {
    target_group_arn = aws_lb_target_group.api.arn
    container_name   = "api"
    container_port   = var.api_container_port
  }

  depends_on = [aws_lb_listener.https]

  tags = merge(local.common_tags, {
    Name = "predictiq-${var.environment}-api-service"
  })
}

# ── AWS Secrets Manager — secrets inventory ────────────────────────────────────
#
# All application secrets are stored in Secrets Manager and injected into the
# ECS task via the `secrets` block above.  No plaintext secrets are passed
# through Terraform environment variables or ECS `environment` blocks.

resource "aws_secretsmanager_secret" "redis_host" {
  name       = "predictiq/${var.environment}/redis-host"
  kms_key_id = aws_kms_key.main.arn
  tags       = merge(local.common_tags, { Name = "predictiq-${var.environment}-redis-host" })
}

resource "aws_secretsmanager_secret_version" "redis_host" {
  secret_id     = aws_secretsmanager_secret.redis_host.id
  secret_string = var.redis_host
}

resource "aws_secretsmanager_secret" "redis_port" {
  name       = "predictiq/${var.environment}/redis-port"
  kms_key_id = aws_kms_key.main.arn
  tags       = merge(local.common_tags, { Name = "predictiq-${var.environment}-redis-port" })
}

resource "aws_secretsmanager_secret_version" "redis_port" {
  secret_id     = aws_secretsmanager_secret.redis_port.id
  secret_string = tostring(var.redis_port)
}

resource "aws_secretsmanager_secret" "redis_auth_token" {
  name        = "predictiq/${var.environment}/redis-auth-token"
  description = "Redis AUTH token. The application assembles REDIS_URL at runtime."
  kms_key_id  = aws_kms_key.main.arn

  tags = merge(local.common_tags, {
    Name            = "predictiq-${var.environment}-redis-auth-token"
    SecretType      = "credential"
    RotationEnabled = var.secret_rotation_lambda_arn != null ? "true" : "false"
  })
}

resource "aws_secretsmanager_secret_version" "redis_auth_token" {
  secret_id     = aws_secretsmanager_secret.redis_auth_token.id
  secret_string = var.redis_auth_token
}

resource "aws_secretsmanager_secret_rotation" "redis_auth_token" {
  count               = var.secret_rotation_lambda_arn != null ? 1 : 0
  secret_id           = aws_secretsmanager_secret.redis_auth_token.id
  rotation_lambda_arn = var.secret_rotation_lambda_arn

  rotation_rules {
    automatically_after_days = 30
  }
}

# ── Database credentials (individual secrets) ────────────────────────────────
# Storing each component separately means the password can be rotated without
# touching the host/name/user, and the assembled connection string (with the
# password in the URL) never appears in Terraform state, ECS task logs, or
# CloudWatch Logs. The application assembles the connection string at runtime
# inside a secrecy::SecretString — see services/api/src/config.rs.

resource "aws_secretsmanager_secret" "db_host" {
  kms_key_id = aws_kms_key.main.arn
  name = "predictiq/${var.environment}/db-host"
  tags = merge(local.common_tags, { Name = "predictiq-${var.environment}-db-host" })
}

resource "aws_secretsmanager_secret_version" "db_host" {
  secret_id     = aws_secretsmanager_secret.db_host.id
  secret_string = var.db_host
}

resource "aws_secretsmanager_secret" "db_port" {
  kms_key_id = aws_kms_key.main.arn
  name = "predictiq/${var.environment}/db-port"
  tags = merge(local.common_tags, { Name = "predictiq-${var.environment}-db-port" })
}

resource "aws_secretsmanager_secret_version" "db_port" {
  secret_id     = aws_secretsmanager_secret.db_port.id
  secret_string = tostring(var.db_port)
}

resource "aws_secretsmanager_secret" "db_name" {
  kms_key_id = aws_kms_key.main.arn
  name = "predictiq/${var.environment}/db-name"
  tags = merge(local.common_tags, { Name = "predictiq-${var.environment}-db-name" })
}

resource "aws_secretsmanager_secret_version" "db_name" {
  secret_id     = aws_secretsmanager_secret.db_name.id
  secret_string = var.db_name
}

resource "aws_secretsmanager_secret" "db_user" {
  kms_key_id = aws_kms_key.main.arn
  name = "predictiq/${var.environment}/db-user"
  tags = merge(local.common_tags, { Name = "predictiq-${var.environment}-db-user" })
}

resource "aws_secretsmanager_secret_version" "db_user" {
  secret_id     = aws_secretsmanager_secret.db_user.id
  secret_string = var.db_user
}

resource "aws_secretsmanager_secret" "db_password" {
  kms_key_id = aws_kms_key.main.arn
  name        = "predictiq/${var.environment}/db-password"
  description = "PostgreSQL password for the predictIQ application user. Never assembled into DATABASE_URL."
  tags = merge(local.common_tags, { Name = "predictiq-${var.environment}-db-password" })
}

resource "aws_secretsmanager_secret_rotation" "db_password" {
  count               = var.secret_rotation_lambda_arn != null ? 1 : 0
  secret_id           = aws_secretsmanager_secret.db_password.id
  rotation_lambda_arn = var.secret_rotation_lambda_arn

  rotation_rules {
    automatically_after_days = 30
  }
}

resource "aws_secretsmanager_secret_version" "db_password" {
  secret_id     = aws_secretsmanager_secret.db_password.id
  secret_string = var.db_password
}

# ── SendGrid ────────────────────────────────────────────────────────────────

resource "aws_secretsmanager_secret" "hmac_key" {
  kms_key_id = aws_kms_key.main.arn
  name        = "predictiq/${var.environment}/hmac-key"
  description = "HMAC secret key used to sign API payloads and webhook signatures"

  tags = merge(local.common_tags, {
    Name            = "predictiq-${var.environment}-hmac-key"
    SecretType      = "signing-key"
    RotationEnabled = "true"
    RotationSchedule = "90-days"
  })
}

resource "aws_secretsmanager_secret_version" "hmac_key" {
  secret_id     = aws_secretsmanager_secret.hmac_key.id
  secret_string = var.hmac_key
}

resource "aws_secretsmanager_secret" "sendgrid_api_key" {
  kms_key_id = aws_kms_key.main.arn
  name        = "predictiq/${var.environment}/sendgrid-api-key"
  description = "SendGrid API key used by the predictIQ API for transactional email. Rotate every 90 days — see docs/runbooks/sendgrid-api-key-rotation.md."

  tags = merge(
    local.common_tags,
    {
      Name = "predictiq-${var.environment}-sendgrid-api-key"
    }
  )
}

resource "aws_secretsmanager_secret_version" "sendgrid_api_key" {
  secret_id     = aws_secretsmanager_secret.sendgrid_api_key.id
  secret_string = var.sendgrid_api_key
}

# Stores only the rotation date (YYYY-MM-DD string) — not the key itself.
# The application reads this at startup to warn if the key is older than 90 days.
resource "aws_secretsmanager_secret" "sendgrid_key_rotated_at" {
  kms_key_id = aws_kms_key.main.arn
  name        = "predictiq/${var.environment}/sendgrid-key-rotated-at"
  description = "ISO-8601 date (YYYY-MM-DD) when SENDGRID_API_KEY was last rotated. Update this alongside the key itself."

  tags = merge(
    local.common_tags,
    {
      Name = "predictiq-${var.environment}-sendgrid-key-rotated-at"
    }
  )
}

resource "aws_secretsmanager_secret_version" "sendgrid_key_rotated_at" {
  secret_id     = aws_secretsmanager_secret.sendgrid_key_rotated_at.id
  secret_string = var.sendgrid_key_rotated_at
}

resource "aws_secretsmanager_secret" "api_signing_key" {
  kms_key_id = aws_kms_key.main.arn
  name        = "predictiq/${var.environment}/api-signing-key"
  description = "Private key used to sign API responses"

  tags = merge(local.common_tags, {
    Name            = "predictiq-${var.environment}-api-signing-key"
    SecretType      = "signing-key"
    RotationEnabled = "true"
    RotationSchedule = "90-days"
  })
}

resource "aws_secretsmanager_secret" "extra" {
  for_each    = local.extra_secrets
  name        = "predictiq/${var.environment}/${replace(each.key, "_", "-")}"
  description = "Injected into the API task as ${each.value.env}"

  tags = merge(local.common_tags, {
    Name = "predictiq-${var.environment}-${replace(each.key, "_", "-")}"
  })
}

resource "aws_secretsmanager_secret_version" "extra" {
  for_each      = local.extra_secrets
  secret_id     = aws_secretsmanager_secret.extra[each.key].id
  secret_string = each.value.value
}

resource "aws_secretsmanager_secret_version" "api_signing_key" {
  secret_id     = aws_secretsmanager_secret.api_signing_key.id
  secret_string = var.api_signing_key
}

# ── IAM — ECS Task Execution Role ─────────────────────────────────────────────


resource "aws_iam_role" "ecs_task_execution_role" {
  name = "predictiq-${var.environment}-ecs-task-execution-role"

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

  tags = merge(local.common_tags, {
    Name = "predictiq-${var.environment}-ecs-task-execution-role"
  })
}

resource "aws_iam_role_policy_attachment" "ecs_task_execution_role_policy" {
  role       = aws_iam_role.ecs_task_execution_role.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
}

# Grant the execution role access to all five Secrets Manager secrets.
resource "aws_iam_role_policy" "ecs_task_execution_secrets" {
  name = "predictiq-${var.environment}-ecs-task-execution-secrets"
  role = aws_iam_role.ecs_task_execution_role.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "secretsmanager:GetSecretValue"
        ]
        Resource = concat([
          aws_secretsmanager_secret.db_host.arn,
          aws_secretsmanager_secret.db_port.arn,
          aws_secretsmanager_secret.db_name.arn,
          aws_secretsmanager_secret.db_user.arn,
          aws_secretsmanager_secret.db_password.arn,
          aws_secretsmanager_secret.redis_host.arn,
          aws_secretsmanager_secret.redis_port.arn,
          aws_secretsmanager_secret.redis_auth_token.arn,
          aws_secretsmanager_secret.hmac_key.arn,
          aws_secretsmanager_secret.sendgrid_api_key.arn,
          aws_secretsmanager_secret.sendgrid_key_rotated_at.arn,
          aws_secretsmanager_secret.api_signing_key.arn,
        ], [for s in aws_secretsmanager_secret.extra : s.arn])
      },
      {
        Effect   = "Allow"
        Action   = ["kms:Decrypt"]
        Resource = [aws_kms_key.main.arn]
      }
    ]
  })
}

# ── IAM — ECS Task Role ────────────────────────────────────────────────────────

resource "aws_iam_role" "ecs_task_role" {
  name = "predictiq-${var.environment}-ecs-task-role"

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

  tags = merge(local.common_tags, {
    Name = "predictiq-${var.environment}-ecs-task-role"
  })
}

# ── Data sources / Outputs ─────────────────────────────────────────────────────

data "aws_region" "current" {}

data "aws_caller_identity" "current" {}

output "kms_key_arn" {
  value = aws_kms_key.main.arn
}

output "cluster_name" {
  value = aws_ecs_cluster.main.name
}

output "service_name" {
  value = aws_ecs_service.api.name
}

output "alb_dns_name" {
  value = aws_lb.main.dns_name
}

# Expose secret ARNs so other modules or CI/CD pipelines can reference them.
output "secret_arns" {
  description = "Map of secret name → ARN for all Secrets Manager secrets managed by this module"
  value = {
    db_host          = aws_secretsmanager_secret.db_host.arn
    db_port          = aws_secretsmanager_secret.db_port.arn
    db_name          = aws_secretsmanager_secret.db_name.arn
    db_user          = aws_secretsmanager_secret.db_user.arn
    db_password      = aws_secretsmanager_secret.db_password.arn
    redis_host       = aws_secretsmanager_secret.redis_host.arn
    redis_port       = aws_secretsmanager_secret.redis_port.arn
    redis_auth_token = aws_secretsmanager_secret.redis_auth_token.arn
    hmac_key         = aws_secretsmanager_secret.hmac_key.arn
    sendgrid_api_key = aws_secretsmanager_secret.sendgrid_api_key.arn
    api_signing_key  = aws_secretsmanager_secret.api_signing_key.arn
  }
  sensitive = true
}
