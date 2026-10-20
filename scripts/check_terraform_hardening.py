#!/usr/bin/env python3
"""Regression check for ALB/ECS/VPC/Redis hardening (issues #1778-#1781)."""
import pathlib
import sys

TF = pathlib.Path(__file__).resolve().parent.parent / "infrastructure" / "terraform" / "modules"

REQUIRED = {
    "ecs/main.tf": [
        "access_logs {",
        "enable_deletion_protection",
        "drop_invalid_header_fields = true",
        "aws_wafv2_web_acl_association",
        "AWSManagedRulesCommonRuleSet",
        "healthCheck",
        "/health/live",
        "stopTimeout",
        "deregistration_delay",
        "unhealthy_threshold = 2",
        "interval            = 15",
    ],
    "vpc/main.tf": ["aws_vpc_endpoint", "aws_flow_log", "ecr.dkr", "secretsmanager", "logs"],
    "redis/main.tf": ["snapshot_retention_limit", "auto_minor_version_upgrade = true", "parameter_group_family"],
}
FORBIDDEN = {"redis/main.tf": ['"default.redis7"']}

errors = []
for rel, needles in REQUIRED.items():
    text = (TF / rel).read_text()
    errors += [f"{rel}: missing `{n}`" for n in needles if n not in text]
for rel, needles in FORBIDDEN.items():
    text = (TF / rel).read_text()
    errors += [f"{rel}: must not contain {n}" for n in needles if n in text]

if errors:
    print("\n".join(errors))
    sys.exit(1)
print("terraform hardening checks passed")
