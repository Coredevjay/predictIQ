variable "environment" {
  type = string
}

variable "ecs_cluster" {
  type = string
}

variable "ecs_service" {
  type = string
}

variable "alb_arn_suffix" {
  type        = string
  description = "aws_lb.arn_suffix (app/<name>/<id>) used as the LoadBalancer dimension"
}

variable "target_group_arn_suffix" {
  type = string
}

variable "rds_instance_id" {
  type = string
}

variable "redis_replication_group" {
  type = string
}

variable "api_desired_count" {
  type    = number
  default = 2
}

variable "rds_free_storage_bytes" {
  type    = number
  default = 5368709120
}

variable "alb_latency_seconds" {
  type    = number
  default = 1
}

locals {
  common_tags = {
    Project   = "predictiq"
    Environment = var.environment
    Owner     = "infrastructure-team"
    ManagedBy = "terraform"
  }
}

resource "aws_cloudwatch_dashboard" "main" {
  dashboard_name = "predictiq-${var.environment}"

  dashboard_body = jsonencode({
    widgets = [
      {
        type = "metric"
        properties = {
          metrics = [
            ["AWS/ECS", "CPUUtilization", "ClusterName", var.ecs_cluster, "ServiceName", var.ecs_service],
            [".", "MemoryUtilization", ".", ".", ".", "."]
          ]
          period = 300
          stat   = "Average"
          region = data.aws_region.current.name
          title  = "ECS Service Metrics"
        }
      },
      {
        type = "metric"
        properties = {
          metrics = [
            ["AWS/ApplicationELB", "TargetResponseTime", "LoadBalancer", var.alb_arn_suffix],
            [".", "RequestCount", ".", "."],
            [".", "HTTPCode_Target_5XX_Count", ".", "."]
          ]
          period = 300
          stat   = "Sum"
          region = data.aws_region.current.name
          title  = "ALB Metrics"
        }
      }
    ]
  })
}

resource "aws_sns_topic" "alerts" {
  name = "predictiq-${var.environment}-alerts"

  tags = merge(
    local.common_tags,
    {
      Name = "predictiq-${var.environment}-alerts"
    }
  )
}

resource "aws_cloudwatch_metric_alarm" "ecs_cpu" {
  alarm_name          = "predictiq-${var.environment}-ecs-cpu-high"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 2
  metric_name         = "CPUUtilization"
  namespace           = "AWS/ECS"
  period              = 300
  statistic           = "Average"
  threshold           = 80
  alarm_description   = "Alert when ECS CPU exceeds 80%"
  alarm_actions       = [aws_sns_topic.alerts.arn]

  dimensions = {
    ClusterName = var.ecs_cluster
    ServiceName = var.ecs_service
  }

  tags = merge(
    local.common_tags,
    {
      Name = "predictiq-${var.environment}-ecs-cpu-high"
    }
  )
}

resource "aws_cloudwatch_metric_alarm" "ecs_memory" {
  alarm_name          = "predictiq-${var.environment}-ecs-memory-high"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 2
  metric_name         = "MemoryUtilization"
  namespace           = "AWS/ECS"
  period              = 300
  statistic           = "Average"
  threshold           = 80
  alarm_description   = "Alert when ECS memory exceeds 80%"
  alarm_actions       = [aws_sns_topic.alerts.arn]

  dimensions = {
    ClusterName = var.ecs_cluster
    ServiceName = var.ecs_service
  }

  tags = merge(
    local.common_tags,
    {
      Name = "predictiq-${var.environment}-ecs-memory-high"
    }
  )
}

resource "aws_cloudwatch_metric_alarm" "alb_5xx" {
  alarm_name          = "predictiq-${var.environment}-alb-5xx-errors"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "HTTPCode_Target_5XX_Count"
  namespace           = "AWS/ApplicationELB"
  period              = 300
  statistic           = "Sum"
  threshold           = 10
  alarm_description   = "Alert when ALB 5XX errors exceed 10"
  alarm_actions       = [aws_sns_topic.alerts.arn]

  dimensions = {
    LoadBalancer = var.alb_arn_suffix
  }

  tags = merge(
    local.common_tags,
    {
      Name = "predictiq-${var.environment}-alb-5xx-errors"
    }
  )
}

locals {
  runbook_base = "docs/runbooks"
  alarms = {
    rds_cpu = {
      namespace = "AWS/RDS", metric = "CPUUtilization", stat = "Average", op = "GreaterThanThreshold"
      threshold = 80, periods = 3, dims = { DBInstanceIdentifier = var.rds_instance_id }
      runbook = "slow-database-queries.md", missing = "missing"
    }
    rds_free_storage = {
      namespace = "AWS/RDS", metric = "FreeStorageSpace", stat = "Minimum", op = "LessThanThreshold"
      threshold = var.rds_free_storage_bytes, periods = 1, dims = { DBInstanceIdentifier = var.rds_instance_id }
      runbook = "critical-database-queries.md", missing = "missing"
    }
    rds_connections = {
      namespace = "AWS/RDS", metric = "DatabaseConnections", stat = "Average", op = "GreaterThanThreshold"
      threshold = 80, periods = 3, dims = { DBInstanceIdentifier = var.rds_instance_id }
      runbook = "high-db-connection-pool-utilization.md", missing = "missing"
    }
    redis_memory = {
      namespace = "AWS/ElastiCache", metric = "DatabaseMemoryUsagePercentage", stat = "Average", op = "GreaterThanThreshold"
      threshold = 80, periods = 3, dims = { ReplicationGroupId = var.redis_replication_group }
      runbook = "redis-failure.md", missing = "missing"
    }
    redis_evictions = {
      namespace = "AWS/ElastiCache", metric = "Evictions", stat = "Sum", op = "GreaterThanThreshold"
      threshold = 100, periods = 2, dims = { ReplicationGroupId = var.redis_replication_group }
      runbook = "low-cache-hit-rate.md", missing = "notBreaching"
    }
    redis_replication_lag = {
      namespace = "AWS/ElastiCache", metric = "ReplicationLag", stat = "Maximum", op = "GreaterThanThreshold"
      threshold = 30, periods = 3, dims = { ReplicationGroupId = var.redis_replication_group }
      runbook = "redis-failure.md", missing = "notBreaching"
    }
    alb_unhealthy_hosts = {
      namespace = "AWS/ApplicationELB", metric = "UnHealthyHostCount", stat = "Maximum", op = "GreaterThanThreshold"
      threshold = 0, periods = 2, dims = { LoadBalancer = var.alb_arn_suffix, TargetGroup = var.target_group_arn_suffix }
      runbook = "service-down.md", missing = "missing"
    }
    ecs_running_tasks = {
      namespace = "ECS/ContainerInsights", metric = "RunningTaskCount", stat = "Minimum", op = "LessThanThreshold"
      threshold = var.api_desired_count, periods = 2, dims = { ClusterName = var.ecs_cluster, ServiceName = var.ecs_service }
      runbook = "ecs-task-crash-loop.md", missing = "breaching"
    }
  }
}

resource "aws_cloudwatch_metric_alarm" "extra" {
  for_each = local.alarms

  alarm_name          = "predictiq-${var.environment}-${replace(each.key, "_", "-")}"
  comparison_operator = each.value.op
  evaluation_periods  = each.value.periods
  metric_name         = each.value.metric
  namespace           = each.value.namespace
  period              = 300
  statistic           = each.value.stat
  threshold           = each.value.threshold
  treat_missing_data  = each.value.missing
  alarm_description   = "${each.key} breached. Runbook: ${local.runbook_base}/${each.value.runbook}"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  dimensions          = each.value.dims

  tags = merge(local.common_tags, { Name = "predictiq-${var.environment}-${each.key}" })
}

resource "aws_cloudwatch_metric_alarm" "alb_latency" {
  alarm_name          = "predictiq-${var.environment}-alb-latency-high"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 3
  metric_name         = "TargetResponseTime"
  namespace           = "AWS/ApplicationELB"
  period              = 300
  extended_statistic  = "p95"
  threshold           = var.alb_latency_seconds
  alarm_description   = "ALB p95 latency high. Runbook: ${local.runbook_base}/high-api-response-time.md"
  alarm_actions       = [aws_sns_topic.alerts.arn]

  dimensions = {
    LoadBalancer = var.alb_arn_suffix
  }

  tags = merge(local.common_tags, { Name = "predictiq-${var.environment}-alb-latency-high" })
}

data "aws_region" "current" {}

output "sns_topic_arn" {
  value = aws_sns_topic.alerts.arn
}
