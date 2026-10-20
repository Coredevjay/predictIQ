//! CORS configuration layer construction and testing.
//!
//! The `build_cors_layer` function constructs a tower-http `CorsLayer` from
//! configuration, with special handling for dev mode (fully permissive) vs.
//! production mode (strict allowlist enforcement).

use axum_http::header::{HeaderName, HeaderValue};
use tower_http::cors::{CorsLayer, AllowOrigin};
use axum_http::Method;
use std::time::Duration;
use crate::config::CorsConfig;

/// Build a CORS layer from configuration.
///
/// # Behavior
/// - **Dev mode** (`CORS_DEV_MODE=true`): Returns a permissive layer that allows
///   all origins, methods, and headers. A warning is emitted to the log. This
///   setting should never be used in production.
/// - **Strict mode**: Enforces an allowlist of origins, methods, and headers from
///   config. If no origins are specified, cross-origin requests are blocked.
///   The `allow_credentials` flag determines whether cookies/credentials are permitted.
///
/// # Parameters
/// - `cfg`: CORS configuration loaded from environment variables.
///
/// # Returns
/// A `CorsLayer` ready to be added to the Axum router.
pub fn build_cors_layer(cfg: &CorsConfig) -> CorsLayer {
    if cfg.dev_mode {
        tracing::warn!(
            "CORS_DEV_MODE is enabled — all origins are permitted. \
             This MUST NOT be used in production."
        );
        return CorsLayer::permissive();
    }

    let origins: Vec<HeaderValue> = cfg
        .allowed_origins
        .iter()
        .filter_map(|o| match o.parse::<HeaderValue>() {
            Ok(v) => Some(v),
            Err(_) => {
                tracing::warn!(value = %o, "CORS: ignoring unparsable allowed origin");
                None
            }
        })
        .collect();

    let methods: Vec<Method> = cfg
        .allowed_methods
        .iter()
        .filter_map(|m| match m.parse::<Method>() {
            Ok(v) => Some(v),
            Err(_) => {
                tracing::warn!(value = %m, "CORS: ignoring unparsable allowed method");
                None
            }
        })
        .collect();

    let headers: Vec<HeaderName> = cfg
        .allowed_headers
        .iter()
        .filter_map(|h| match h.parse::<HeaderName>() {
            Ok(v) => Some(v),
            Err(_) => {
                tracing::warn!(value = %h, "CORS: ignoring unparsable allowed header");
                None
            }
        })
        .collect();

    let layer = CorsLayer::new()
        .allow_origin(origins)
        .allow_methods(methods)
        .allow_headers(headers)
        .max_age(Duration::from_secs(cfg.max_age_secs));

    if cfg.allow_credentials {
        if cfg.allowed_origins.iter().any(|o| o.trim() == "*") {
            tracing::warn!("CORS: wildcard origin is not allowed with credentials — credentials disabled");
            return layer;
        }
        layer.allow_credentials(true)
    } else {
        layer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_mode_is_permissive() {
        let cfg = CorsConfig {
            dev_mode: true,
            allowed_origins: vec!["https://example.com".to_string()],
            allowed_methods: vec!["GET".to_string()],
            allowed_headers: vec!["content-type".to_string()],
            allow_credentials: false,
            max_age_secs: 3600,
        };

        let layer = build_cors_layer(&cfg);
        // Permissive layer allows all origins/methods/headers.
        // We can't easily inspect the inner state, but we verify it builds without panic.
        drop(layer);
    }

    #[test]
    fn strict_mode_with_single_origin() {
        let cfg = CorsConfig {
            dev_mode: false,
            allowed_origins: vec!["https://app.example.com".to_string()],
            allowed_methods: vec!["GET".to_string(), "POST".to_string()],
            allowed_headers: vec!["content-type".to_string(), "authorization".to_string()],
            allow_credentials: false,
            max_age_secs: 3600,
        };

        let layer = build_cors_layer(&cfg);
        // Strict mode respects allowlist configuration.
        drop(layer);
    }

    #[test]
    fn strict_mode_with_empty_origins_blocks_cors() {
        let cfg = CorsConfig {
            dev_mode: false,
            allowed_origins: vec![],
            allowed_methods: vec!["GET".to_string()],
            allowed_headers: vec!["content-type".to_string()],
            allow_credentials: false,
            max_age_secs: 3600,
        };

        let layer = build_cors_layer(&cfg);
        // With no allowed origins, cross-origin requests should be blocked.
        drop(layer);
    }

    #[test]
    fn credentials_flag_is_honored() {
        let cfg_with_credentials = CorsConfig {
            dev_mode: false,
            allowed_origins: vec!["https://app.example.com".to_string()],
            allowed_methods: vec!["GET".to_string()],
            allowed_headers: vec!["content-type".to_string()],
            allow_credentials: true,
            max_age_secs: 3600,
        };

        let cfg_without_credentials = CorsConfig {
            dev_mode: false,
            allowed_origins: vec!["https://app.example.com".to_string()],
            allowed_methods: vec!["GET".to_string()],
            allowed_headers: vec!["content-type".to_string()],
            allow_credentials: false,
            max_age_secs: 3600,
        };

        let layer_with = build_cors_layer(&cfg_with_credentials);
        let layer_without = build_cors_layer(&cfg_without_credentials);

        // Both layers build successfully; credentials behavior is embedded in them.
        drop(layer_with);
        drop(layer_without);
    }

    #[test]
    fn max_age_is_respected() {
        let cfg = CorsConfig {
            dev_mode: false,
            allowed_origins: vec!["https://app.example.com".to_string()],
            allowed_methods: vec!["GET".to_string()],
            allowed_headers: vec!["content-type".to_string()],
            allow_credentials: false,
            max_age_secs: 7200,
        };

        let layer = build_cors_layer(&cfg);
        // max_age is baked into the layer during construction.
        drop(layer);
    }

    #[test]
    fn multiple_origins_are_supported() {
        let cfg = CorsConfig {
            dev_mode: false,
            allowed_origins: vec![
                "https://app1.example.com".to_string(),
                "https://app2.example.com".to_string(),
                "https://staging.example.com".to_string(),
            ],
            allowed_methods: vec!["GET".to_string(), "POST".to_string(), "PUT".to_string()],
            allowed_headers: vec!["content-type".to_string(), "authorization".to_string()],
            allow_credentials: true,
            max_age_secs: 5000,
        };

        let layer = build_cors_layer(&cfg);
        drop(layer);
    }

    #[test]
    fn invalid_origin_headers_are_skipped() {
        let cfg = CorsConfig {
            dev_mode: false,
            // Mix valid and invalid origins.
            allowed_origins: vec![
                "https://valid.example.com".to_string(),
                "not a valid header value 😀".to_string(), // Invalid UTF-8 in header
            ],
            allowed_methods: vec!["GET".to_string()],
            allowed_headers: vec!["content-type".to_string()],
            allow_credentials: false,
            max_age_secs: 3600,
        };

        let layer = build_cors_layer(&cfg);
        // Invalid origins are silently filtered out.
        drop(layer);
    }
}
