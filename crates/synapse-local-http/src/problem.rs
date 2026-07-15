use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Problem {
    #[serde(rename = "type")]
    pub problem_type: String,
    pub title: String,
    pub status: u16,
    pub code: String,
    pub detail: String,
    pub request_id: String,
    pub retryable: bool,
}

pub(crate) fn problem_response(
    status: StatusCode,
    code: &str,
    title: &str,
    detail: &str,
    request_id: String,
    retryable: bool,
) -> Response {
    let body = Problem {
        problem_type: format!("urn:synapsegit:error:{code}"),
        title: title.to_owned(),
        status: status.as_u16(),
        code: code.to_owned(),
        detail: detail.to_owned(),
        request_id,
        retryable,
    };
    let mut response = (status, Json(body)).into_response();
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/problem+json"),
    );
    response
}
