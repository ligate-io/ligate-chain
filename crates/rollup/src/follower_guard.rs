//! Axum middleware that blocks transaction submission when the node
//! is running in [`crate::NodeRole::Follower`].
//!
//! ## Why
//!
//! In follower mode the node still mounts the full chain REST surface
//! (so users can hit `GET /v1/ledger/...`, `GET /v1/rollup/info`,
//! etc. against a local follower without bouncing to the public
//! sequencer). But the *submission* endpoint, `POST /v1/sequencer/txs`,
//! must not silently accept a transaction that has nowhere to go. The
//! follower's DA address isn't in the registry, so any blob it tried
//! to post would be dropped at the STF level; without this guard the
//! tx would queue locally, get dropped, and the user would never know
//! why their tx didn't appear on chain.
//!
//! Returning `503 Service Unavailable` with a clear "this node is a
//! follower; submit to the upstream sequencer" message gives the user
//! an obvious signal to redirect their submission.
//!
//! ## What it does
//!
//! Intercepts `POST` requests whose path ends in `/sequencer/txs`
//! (matches both `/sequencer/txs` if mounted at root and
//! `/v1/sequencer/txs` once the chain repo's `/v1` nesting is
//! applied) and returns 503 with a JSON body matching the SDK's
//! `ApiError` shape. Other requests pass through unchanged.
//!
//! Layered onto the master axum router in
//! `mock_rollup::create_endpoints` and `celestia_rollup::create_endpoints`
//! when [`crate::NodeRole`] is `Follower`. In `Sequencer` mode the
//! middleware is not applied at all — zero runtime overhead for the
//! common case.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// 503 message returned when a follower receives a submission. JSON
/// shape matches the SDK's `ApiError` so existing clients deserialise
/// it with their normal error path.
const MESSAGE: &str = r#"{"status":503,"message":"This node is running in follower mode. Transaction submission is disabled. Submit to the upstream sequencer (e.g. https://rpc.ligate.io) and read state from this follower locally.","details":{}}"#;

/// Middleware that intercepts `POST /...sequencer/txs` requests with
/// a 503 response. Wire via `axum::middleware::from_fn` only when the
/// node is in follower mode.
pub async fn block_sequencer_submission(req: Request<Body>, next: Next) -> Response {
    if req.method() == axum::http::Method::POST && req.uri().path().ends_with("/sequencer/txs") {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            MESSAGE,
        )
            .into_response();
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request};
    use axum::middleware;
    use axum::routing::{get, post};
    use axum::Router;
    use http_body_util::BodyExt as _;
    use tower::ServiceExt as _;

    /// A handler that stand-ins for the real sequencer's accept_tx —
    /// returns 200 if reached, so the middleware blocking it shows up
    /// as a 503 in the test response.
    async fn would_accept_tx() -> &'static str {
        "would accept"
    }

    fn router_with_follower_guard() -> Router {
        Router::new()
            .route("/v1/sequencer/txs", post(would_accept_tx))
            .route("/v1/sequencer/ready", get(|| async { "ready" }))
            .route("/v1/ledger/slots/latest", get(|| async { "{}" }))
            .layer(middleware::from_fn(block_sequencer_submission))
    }

    async fn body_string(resp: Response) -> String {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn post_sequencer_txs_is_blocked_with_503() {
        let app = router_with_follower_guard();
        let req = Request::builder()
            .method(Method::POST)
            .uri("/v1/sequencer/txs")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = body_string(resp).await;
        assert!(body.contains("follower mode"));
        assert!(body.contains("submit to the upstream sequencer") || body.contains("Submit to"));
    }

    #[tokio::test]
    async fn get_sequencer_ready_passes_through() {
        let app = router_with_follower_guard();
        let req = Request::builder()
            .method(Method::GET)
            .uri("/v1/sequencer/ready")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(body_string(resp).await, "ready");
    }

    #[tokio::test]
    async fn get_ledger_slots_latest_passes_through() {
        let app = router_with_follower_guard();
        let req = Request::builder()
            .method(Method::GET)
            .uri("/v1/ledger/slots/latest")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn post_to_other_path_passes_through() {
        // Even POST methods, as long as the path doesn't end in
        // `/sequencer/txs`, are not affected. (None exist in our
        // surface today; this test documents intent for future routes.)
        let extra =
            router_with_follower_guard().route("/v1/some/other/post", post(would_accept_tx));
        let req = Request::builder()
            .method(Method::POST)
            .uri("/v1/some/other/post")
            .body(Body::empty())
            .unwrap();
        let resp = extra.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
