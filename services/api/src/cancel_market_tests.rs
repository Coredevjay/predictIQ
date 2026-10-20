#[cfg(test)]
mod cancel_market_tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        routing::post,
        Router,
    };
    use std::sync::Arc;
    use tower::ServiceExt;

    use crate::handlers::{cancel_market, InvalidationResult};

    // ---------------------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------------------

    /// Build a minimal router wired to `cancel_market` for handler-level tests.
    fn app(state: Arc<crate::AppState>) -> Router {
        Router::new()
            .route("/admin/markets/:market_id/cancel", post(cancel_market))
            .with_state(state)
    }

    async fn post_cancel(
        router: Router,
        market_id: i64,
    ) -> axum::response::Response {
        router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/admin/markets/{market_id}/cancel"))
                    .header("content-type", "application/json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    // ---------------------------------------------------------------------------
    // Unit tests — no real DB/Redis required
    // ---------------------------------------------------------------------------

    /// Verifies that a successful cancel DB write returns 200 and a non-zero
    /// `invalidated_keys` count.
    #[tokio::test]
    #[ignore] // Requires PostgreSQL + Redis
    async fn test_cancel_market_success_returns_200_and_invalidation_count() {
        let state = build_test_state().await;
        // Insert a test market first so the UPDATE finds a row.
        sqlx::query(
            "INSERT INTO markets (id, title, status, total_volume, ends_at) \
             VALUES (9003, 'Cancel Test Market', 'active', 0, NOW() + INTERVAL '1 day')",
        )
        .execute(state.db.pool())
        .await
        .unwrap();

        let response = post_cancel(app(Arc::clone(&state)), 9003).await;
        assert_eq!(response.status(), StatusCode::OK);

        let body: InvalidationResult =
            serde_json::from_slice(&axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert!(body.invalidated_keys > 0);

        // Cleanup
        sqlx::query("DELETE FROM markets WHERE id = 9003")
            .execute(state.db.pool())
            .await
            .unwrap();
    }

    /// Verifies that cancelling a non-existent market returns 500.
    #[tokio::test]
    #[ignore] // Requires PostgreSQL + Redis
    async fn test_cancel_market_not_found_returns_500() {
        let state = build_test_state().await;
        let response = post_cancel(app(Arc::clone(&state)), 999_999_999).await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    /// Verifies that cancelling an already-cancelled market returns 500.
    #[tokio::test]
    #[ignore] // Requires PostgreSQL + Redis
    async fn test_cancel_market_already_cancelled_returns_500() {
        let state = build_test_state().await;
        sqlx::query(
            "INSERT INTO markets (id, title, status, total_volume, ends_at) \
             VALUES (9004, 'Cancelled Market', 'cancelled', 0, NOW() - INTERVAL '1 day')",
        )
        .execute(state.db.pool())
        .await
        .unwrap();

        let response = post_cancel(app(Arc::clone(&state)), 9004).await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        // Cleanup
        sqlx::query("DELETE FROM markets WHERE id = 9004")
            .execute(state.db.pool())
            .await
            .unwrap();
    }

    // ---------------------------------------------------------------------------
    // Pure-logic unit tests (no I/O)
    // ---------------------------------------------------------------------------

    /// Verifies that `InvalidationResult` from cancel serialises correctly.
    #[test]
    fn test_cancel_invalidation_result_serialises() {
        let result = InvalidationResult { invalidated_keys: 6, cache_invalidated: true };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["invalidated_keys"], 6);
        assert_eq!(json["cache_invalidated"], true);
    }

    // ---------------------------------------------------------------------------
    // Helper — builds AppState from env (used by #[ignore] integration tests)
    // ---------------------------------------------------------------------------

    #[cfg(test)]
    async fn build_test_state() -> Arc<crate::AppState> {
        use crate::{
            audit::AuditLogger,
            blockchain::BlockchainClient,
            cache::RedisCache,
            config::Config,
            db::Database,
            email::{queue::EmailQueue, service::EmailService, webhook::WebhookHandler},
            metrics::Metrics,
            newsletter::IpRateLimiter,
        };

        let config = Config::from_env();
        let metrics = Metrics::new().expect("metrics");
        let cache = RedisCache::new(&config.redis_url).await.expect("redis");
        let db = Database::new(&config.database_url, cache.clone(), metrics.clone(), &config.db_pool)
            .await
            .expect("db");
        let blockchain = BlockchainClient::new(&config, cache.clone(), db.clone(), metrics.clone())
            .expect("blockchain");
        let email_service = EmailService::new(config.clone()).expect("email_service");
        let email_queue = EmailQueue::new(cache.clone(), db.clone());
        let webhook_handler = WebhookHandler::new(db.clone(), cache.clone(), config.webhook_replay_window_secs);
        let audit_logger = AuditLogger::new(db.pool());

        Arc::new(crate::AppState {
            config,
            cache: cache.clone(),
            db,
            blockchain,
            metrics,
            newsletter_rate_limiter: IpRateLimiter::new(cache),
            email_service,
            email_queue,
            webhook_handler,
            audit_logger,
        })
    }
}
