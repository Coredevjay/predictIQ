#!/usr/bin/env bash
# Regression checks for issues #1775, #1782, #1783, #1784.
set -euo pipefail
cd "$(dirname "$0")/../modules"
fail() { echo "FAIL: $1" >&2; exit 1; }

grep -q 'aws_sns_topic_subscription' monitoring/main.tf || fail "alerts SNS topic has no subscriptions (#1775)"
! grep -q 'engine_version *= *"15\.3"' rds/main.tf || fail "RDS pins outdated minor version (#1782)"
! grep -q 'timestamp()' rds/main.tf || fail "RDS uses timestamp() (#1782)"
grep -q 'performance_insights_enabled *= *true' rds/main.tf || fail "RDS Performance Insights off (#1782)"
grep -q 'enabled_cloudwatch_logs_exports' rds/main.tf || fail "RDS log exports unset (#1782)"
! grep -qE 'output "(database_url|redis_url)"' rds/main.tf redis/main.tf || fail "URL outputs embed passwords (#1783)"
grep -q 'kms_key_id' ecs/main.tf || fail "ECS secrets/logs lack KMS (#1784)"
grep -q 'enable_key_rotation *= *true' ecs/main.tf || fail "KMS key rotation off (#1784)"
echo "OK"
