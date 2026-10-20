output "redis_url" {
  value     = "rediss://:${var.redis_auth_token}@${aws_elasticache_replication_group.main.primary_endpoint_address}:6379"
  sensitive = true
}

output "port" {
  value = aws_elasticache_replication_group.main.port
}

output "auth_token" {
  value     = var.redis_auth_token
  sensitive = true
}

output "replication_group_id" {
  value = aws_elasticache_replication_group.main.id
}
