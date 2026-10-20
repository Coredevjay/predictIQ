use utoipa::OpenApi;

use crate::handlers::{
    ApiError, AuditLogsQuery, AuditStatisticsQuery, EmailAnalyticsQuery, EmailTestRequest,
    FeaturedMarketView, InvalidationResult, NewsletterEmailRequest, NewsletterExportBody,
    NewsletterExportResponse, NewsletterGdprTokenRequest, NewsletterResponse,
    NewsletterSubscribeRequest, ResolveMarketRequest,
    NewsletterConfirmQuery, NewsletterUnsubscribeQuery, NewsletterExportQuery,
};
use crate::pagination::PaginationQuery;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "PredictIQ API",
        version = "1.0.0",
        description = "REST API for the PredictIQ prediction markets platform.\n\
            \n\
            ## API Versioning\n\
            The API uses URL path versioning (`/api/v1/`). The current stable version is **v1**.\n\
            \n\
            ## Deprecation Policy\n\
            When a version is deprecated, responses include a `Deprecation` header. \
            Deprecated versions are supported for a minimum of 12 months.",
    ),
    paths(
        crate::handlers::health,
        crate::handlers::newsletter_subscribe,
        crate::handlers::newsletter_confirm,
        crate::handlers::newsletter_unsubscribe,
        crate::handlers::newsletter_gdpr_request_token,
        crate::handlers::newsletter_gdpr_export,
        crate::handlers::newsletter_gdpr_delete,
        crate::handlers::statistics,
        crate::handlers::featured_markets,
        crate::handlers::content,
        crate::handlers::resolve_market,
        crate::handlers::cancel_market,
        crate::handlers::blockchain_health,
        crate::handlers::blockchain_market_data,
        crate::handlers::blockchain_platform_stats,
        crate::handlers::blockchain_user_bets,
        crate::handlers::blockchain_oracle_result,
        crate::handlers::blockchain_tx_status,
        crate::handlers::blockchain_replay,
        crate::handlers::email_preview,
        crate::handlers::email_send_test,
        crate::handlers::email_analytics,
        crate::handlers::email_queue_stats,
        crate::handlers::email_dead_letter_list,
        crate::handlers::email_dead_letter_requeue,
        crate::handlers::sendgrid_webhook,
        crate::handlers::audit_logs,
        crate::handlers::audit_statistics,
    ),
    components(
        schemas(
            ApiError,
            FeaturedMarketView,
            InvalidationResult,
            NewsletterSubscribeRequest,
            NewsletterEmailRequest,
            NewsletterGdprTokenRequest,
            NewsletterExportBody,
            NewsletterResponse,
            NewsletterExportResponse,
            ResolveMarketRequest,
            EmailTestRequest,
        )
    ),
    tags(
        (name = "health", description = "Health check"),
        (name = "newsletter", description = "Newsletter subscription management"),
        (name = "markets", description = "Market data and resolution"),
        (name = "blockchain", description = "Stellar blockchain integration"),
        (name = "email", description = "Email service management (admin)"),
        (name = "webhooks", description = "Incoming provider webhooks"),
        (name = "audit", description = "Audit log access (admin)"),
    ),
    security(
        ("api_key" = [])
    )
)]
pub struct ApiDoc;

#[cfg(test)]
mod tests {
    use super::*;
    use utoipa::openapi::OpenApi as OpenApiDoc;

    fn build_spec() -> OpenApiDoc {
        ApiDoc::openapi()
    }

    #[test]
    fn spec_includes_expected_paths() {
        let spec = build_spec();
        let paths = &spec.paths.paths;

        let expected = [
            "/health",
            "/api/v1/newsletter/subscribe",
            "/api/v1/newsletter/confirm",
            "/api/v1/newsletter/unsubscribe",
            "/api/v1/statistics",
            "/api/v1/markets/featured",
            "/api/v1/markets/resolve",
            "/api/v1/blockchain/health",
            "/api/v1/blockchain/replay",
            "/api/v1/email/preview",
            "/api/v1/email/send-test",
            "/api/v1/email/analytics",
            "/api/v1/webhooks/sendgrid",
            "/api/v1/audit/logs",
            "/api/v1/audit/statistics",
        ];

        for path in expected {
            assert!(
                paths.contains_key(path),
                "expected OpenAPI spec to contain path `{path}`, found: {:?}",
                paths.keys().collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn spec_includes_expected_security_schemes() {
        let spec = build_spec();
        let components = spec
            .components
            .as_ref()
            .expect("generated spec should define components");

        assert!(
            components.security_schemes.contains_key("api_key"),
            "expected `api_key` security scheme, found: {:?}",
            components.security_schemes.keys().collect::<Vec<_>>()
        );

        let security = spec
            .security
            .as_ref()
            .expect("generated spec should declare a global security requirement");
        assert!(
            security
                .iter()
                .any(|req| req.contains_key("api_key")),
            "expected global security requirement to reference `api_key`"
        );
    }

    #[test]
    fn spec_includes_expected_server_url() {
        let spec = build_spec();
        let servers = spec
            .servers
            .as_ref()
            .expect("generated spec should declare at least one server");

        assert!(
            servers.iter().any(|s| s.url == "/api/v1"),
            "expected server URL `/api/v1`, found: {:?}",
            servers.iter().map(|s| s.url.clone()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn spec_includes_expected_schemas() {
        let spec = build_spec();
        let components = spec
            .components
            .as_ref()
            .expect("generated spec should define components");

        for schema in ["ApiError", "FeaturedMarketView", "NewsletterResponse"] {
            assert!(
                components.schemas.contains_key(schema),
                "expected schema `{schema}`, found: {:?}",
                components.schemas.keys().collect::<Vec<_>>()
            );
        }
    }
}
