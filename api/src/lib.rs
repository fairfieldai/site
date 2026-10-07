//! HTTP API served by API Gateway and CloudFront under `/api`.
//!
//! CloudFront forwards the full request path, so every route is mounted under
//! `/api`.

use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

#[derive(Debug, Serialize)]
struct Status {
    status: &'static str,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: &'static str,
}

pub fn router() -> Router {
    Router::new()
        .route("/api/health", get(health))
        .fallback(not_found)
}

async fn health() -> Json<Status> {
    Json(Status { status: "ok" })
}

async fn not_found() -> (StatusCode, Json<ErrorBody>) {
    (
        StatusCode::NOT_FOUND,
        Json(ErrorBody { error: "not found" }),
    )
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::router;

    async fn send(method: Method, uri: &str) -> (StatusCode, Option<Value>) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        let response = router().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).ok())
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let (status, body) = send(Method::GET, "/api/health").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, Some(json!({ "status": "ok" })));
    }

    #[tokio::test]
    async fn unknown_route_returns_json_404() {
        let (status, body) = send(Method::GET, "/api/missing").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body, Some(json!({ "error": "not found" })));
    }

    #[tokio::test]
    async fn routes_outside_api_prefix_are_not_found() {
        let (status, _) = send(Method::GET, "/health").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn wrong_method_is_rejected() {
        let (status, _) = send(Method::POST, "/api/health").await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    }
}
