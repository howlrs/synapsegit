use crate::problem::problem_response;
use axum::extract::{Request, State};
use axum::http::header::{CACHE_CONTROL, HOST, ORIGIN};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const TOKEN_HEADER: &str = "x-synapse-local-token";
#[derive(Clone)]
pub(crate) struct SecurityPolicy {
    expected_host: Arc<str>,
    canonical_origin: Arc<str>,
    token: Arc<str>,
    server_instance: Arc<str>,
    next_request: Arc<AtomicU64>,
}

impl SecurityPolicy {
    pub(crate) fn new(port: u16, token: String, server_instance: String) -> Self {
        let (expected_host, canonical_origin) = if port == 80 {
            ("127.0.0.1".to_owned(), "http://127.0.0.1".to_owned())
        } else {
            (
                format!("127.0.0.1:{port}"),
                format!("http://127.0.0.1:{port}"),
            )
        };
        Self {
            expected_host: expected_host.into(),
            canonical_origin: canonical_origin.into(),
            token: token.into(),
            server_instance: server_instance.into(),
            next_request: Arc::new(AtomicU64::new(1)),
        }
    }

    pub(crate) fn token(&self) -> &str {
        &self.token
    }

    pub(crate) fn canonical_origin(&self) -> &str {
        &self.canonical_origin
    }

    pub(crate) fn server_instance(&self) -> &str {
        &self.server_instance
    }

    pub(crate) fn request_id(&self) -> String {
        let sequence = self.next_request.fetch_add(1, Ordering::Relaxed);
        format!("{}-{sequence}", self.server_instance)
    }
}

pub(crate) async fn enforce_local_request(
    State(policy): State<SecurityPolicy>,
    request: Request,
    next: Next,
) -> Response {
    let request_id = policy.request_id();
    let headers = request.headers();
    let path = request.uri().path().to_owned();

    let denied = duplicate_or_noncanonical_header(headers, HOST, &policy.expected_host)
        || contains_proxy_header(headers)
        || (path.starts_with("/api/v1")
            && path != "/api/v1/health"
            && !single_header_matches(headers, TOKEN_HEADER, &policy.token))
        || (path.starts_with("/api/v1")
            && request.uri().query().is_some()
            && !path.ends_with("/reflog"))
        || (matches!(*request.method(), Method::GET | Method::HEAD) && has_request_body(headers))
        || request.method() == Method::OPTIONS
        || (is_unsafe(request.method())
            && (!single_header_matches(headers, ORIGIN, &policy.canonical_origin)
                || headers
                    .get("sec-fetch-site")
                    .is_some_and(|value| value.as_bytes() != b"same-origin")));

    let mut response = if denied {
        problem_response(
            StatusCode::FORBIDDEN,
            "local_request_denied",
            "Local request denied",
            "The request did not satisfy the local browser security policy.",
            request_id,
            false,
        )
    } else {
        next.run(request).await
    };
    apply_response_policy(&path, response.headers_mut());
    response
}

fn contains_proxy_header(headers: &HeaderMap) -> bool {
    headers.keys().any(|name| {
        let name = name.as_str();
        matches!(name, "forwarded" | "x-forwarded" | "x-original-host")
            || name.starts_with("x-forwarded-")
    })
}

fn duplicate_or_noncanonical_header(headers: &HeaderMap, name: HeaderName, expected: &str) -> bool {
    !single_header_matches(headers, name, expected)
}

fn single_header_matches(
    headers: &HeaderMap,
    name: impl axum::http::header::AsHeaderName,
    expected: &str,
) -> bool {
    let mut values = headers.get_all(name).iter();
    let Some(value) = values.next() else {
        return false;
    };
    values.next().is_none() && constant_time_equal(value.as_bytes(), expected.as_bytes())
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn is_unsafe(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

fn has_request_body(headers: &HeaderMap) -> bool {
    if headers.contains_key("transfer-encoding") {
        return true;
    }
    let mut lengths = headers.get_all("content-length").iter();
    let Some(length) = lengths.next() else {
        return false;
    };
    lengths.next().is_some() || length.as_bytes() != b"0"
}

fn apply_response_policy(path: &str, headers: &mut HeaderMap) {
    let cache = if path.starts_with("/assets/") {
        "public, max-age=0, must-revalidate"
    } else {
        "no-store"
    };
    headers.insert(CACHE_CONTROL, HeaderValue::from_static(cache));
    for (name, value) in [
        (
            "content-security-policy",
            "default-src 'none'; base-uri 'none'; connect-src 'self'; form-action 'self'; frame-ancestors 'none'; img-src 'self' blob:; object-src 'none'; script-src 'self'; style-src 'self'",
        ),
        ("cross-origin-opener-policy", "same-origin"),
        ("cross-origin-resource-policy", "same-origin"),
        (
            "permissions-policy",
            "camera=(), geolocation=(), microphone=()",
        ),
        ("referrer-policy", "no-referrer"),
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "DENY"),
    ] {
        headers.insert(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
}
