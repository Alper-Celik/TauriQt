// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

//! Custom-scheme bridge payloads: the encoding of the `(method, url, headers,
//! body)` block Qt hands the runtime, and of the response it hands back.
//!
//! QtWebEngine never delivers a custom-protocol POST body, so this runtime's
//! invoke system carries it in the `X-Tauriqt-Payload` header (base64) and
//! [`decode_request`] turns it back into an ordinary `http::Request` before
//! tauri's IPC handler sees it. The `extern "Rust"` callbacks and the cxx bridge
//! themselves stay in `tauri-runtime-qt/src/qt.rs`; only the byte-level rules
//! live here.
//!
//! The header block is newline-separated (`k: v` per line): the C++ side reads
//! and writes exactly this shape, so both ends of the bridge share one encoding.

use std::borrow::Cow;

use base64::Engine as _;

/// Decodes an `X-Tauriqt-Payload` header value (standard base64, padding
/// required). An error means "not a payload" — the runtime then falls back to
/// the raw body rather than failing the request.
pub fn decode_payload(value: &str) -> Result<Vec<u8>, base64::DecodeError> {
    base64::engine::general_purpose::STANDARD.decode(value)
}

/// Drops CR/LF from a header value: in the newline-separated block they would
/// terminate the header line early (and inject a second one).
pub fn sanitize_header_value(value: &str) -> String {
    value.replace(['\r', '\n'], "")
}

/// Turns the bridge's `(method, url, headers, body)` into an `http::Request`.
///
/// * lines without the `": "` separator are ignored (they cannot be headers);
/// * a `X-Tauriqt-Payload` header (base64, case-insensitive name) *replaces*
///   the body and is not forwarded;
/// * an invalid base64 payload falls back to the raw body instead of failing —
///   a request that reached us is worth dispatching;
/// * `None` means the pieces do not form a request at all (unknown method,
///   malformed URL, invalid header), which the runtime reports as unhandled.
pub fn decode_request(
    method: &str,
    url: &str,
    headers: &str,
    body: &str,
) -> Option<http::Request<Vec<u8>>> {
    let mut payload: Option<Vec<u8>> = None;
    let mut request = http::Request::builder().method(method).uri(url);
    for line in headers.lines() {
        let Some((name, value)) = line.split_once(": ") else {
            continue;
        };
        let (name, value) = (name.trim(), value.trim());
        if name.eq_ignore_ascii_case("x-tauriqt-payload") {
            payload = decode_payload(value).ok();
            continue;
        }
        request = request.header(name, value);
    }
    let request_body = payload.unwrap_or_else(|| body.as_bytes().to_vec());
    request.body(request_body).ok()
}

/// Encodes an `http::Response` into the header/body representation the bridge
/// uses: one `k: v` line per UTF-8 header (non-UTF-8 values are dropped), the
/// CORS expose header injected when the block does not already contain that
/// spelling, and the body as a lossy UTF-8 string.
pub fn encode_response(response: http::Response<Cow<'static, [u8]>>) -> (i32, String, String) {
    let status = response.status().as_u16() as i32;
    let mut headers = String::new();
    for (name, value) in response.headers() {
        if let Ok(value) = value.to_str() {
            headers.push_str(name.as_str());
            headers.push_str(": ");
            headers.push_str(&sanitize_header_value(value));
            headers.push('\n');
        }
    }
    if !headers.contains("Access-Control-Expose-Headers") {
        headers.push_str("Access-Control-Expose-Headers: Tauri-Response, Content-Type\n");
    }
    (
        status,
        headers,
        String::from_utf8_lossy(response.body()).into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const CORS: &str = "Access-Control-Expose-Headers: Tauri-Response, Content-Type\n";

    fn response(
        status: u16,
        headers: &[(&'static str, &'static str)],
        body: &'static [u8],
    ) -> http::Response<Cow<'static, [u8]>> {
        let mut response = http::Response::new(Cow::Borrowed(body));
        *response.status_mut() = http::StatusCode::from_u16(status).expect("valid status");
        for (name, value) in headers {
            response.headers_mut().insert(
                http::HeaderName::from_static(name),
                http::HeaderValue::from_static(value),
            );
        }
        response
    }

    #[test]
    fn encode_response_joins_headers_line_by_line() {
        let (status, headers, body) = encode_response(response(
            200,
            &[("content-type", "text/plain"), ("x-a", "1")],
            b"hello",
        ));
        assert_eq!(status, 200);
        assert_eq!(headers, format!("content-type: text/plain\nx-a: 1\n{CORS}"));
        assert_eq!(body, "hello");
    }

    #[test]
    fn encode_response_injects_the_cors_header_when_absent() {
        let (status, headers, body) = encode_response(response(404, &[], b""));
        assert_eq!(status, 404);
        assert_eq!(headers, CORS);
        assert_eq!(body, "");
    }

    #[test]
    fn encode_response_keeps_an_existing_expose_header() {
        // `http` lower-cases header names, so the (case-sensitive) contains()
        // check never sees an explicit header by that spelling — the injection
        // stays. Only a *value* spelling it out suppresses it.
        let (_, headers, _) = encode_response(response(
            200,
            &[("access-control-expose-headers", "X-Tauri-Response")],
            b"",
        ));
        assert_eq!(
            headers,
            format!("access-control-expose-headers: X-Tauri-Response\n{CORS}")
        );

        let (_, headers, _) = encode_response(response(
            200,
            &[("x-hint", "Access-Control-Expose-Headers")],
            b"",
        ));
        assert_eq!(headers, "x-hint: Access-Control-Expose-Headers\n");
    }

    #[test]
    fn sanitize_header_value_drops_cr_and_lf() {
        assert_eq!(sanitize_header_value("a\r\nb"), "ab");
        assert_eq!(sanitize_header_value("plain"), "plain");
    }

    #[test]
    fn encode_response_skips_values_that_are_not_utf8() {
        let mut response = http::Response::new(Cow::Borrowed(&b""[..]));
        response.headers_mut().insert(
            http::HeaderName::from_static("x-bin"),
            http::HeaderValue::from_bytes(b"\xff").expect("obs-text is a valid header value"),
        );
        let (_, headers, _) = encode_response(response);
        assert_eq!(headers, CORS);
    }

    #[test]
    fn encode_response_renders_a_non_utf8_body_lossily() {
        let (_, _, body) = encode_response(response(200, &[], b"a\xffb"));
        assert_eq!(body, "a\u{fffd}b");
    }

    #[test]
    fn payload_decoding_is_standard_base64() {
        assert_eq!(decode_payload("aGVsbG8=").expect("valid base64"), b"hello");
        assert_eq!(
            decode_payload("").expect("empty decodes to nothing"),
            Vec::<u8>::new()
        );
        assert!(decode_payload("a").is_err());
        assert!(decode_payload("aGVsbG8").is_err());
        assert!(decode_payload("!!!").is_err());
    }

    #[test]
    fn decode_request_keeps_headers_and_body() {
        let request = decode_request(
            "GET",
            "tauri://localhost/index.html",
            "accept: text/html\nx-a: 1\n",
            "raw",
        )
        .expect("usable request");
        assert_eq!(request.method(), "GET");
        assert_eq!(request.uri(), "tauri://localhost/index.html");
        assert_eq!(request.headers()["accept"], "text/html");
        assert_eq!(request.headers()["x-a"], "1");
        assert_eq!(request.body(), b"raw");
    }

    #[test]
    fn decode_request_ignores_lines_without_a_separator() {
        let request = decode_request(
            "GET",
            "tauri://localhost/",
            "not-a-header\r\naccept: */*\n",
            "",
        )
        .expect("usable request");
        assert_eq!(request.headers()["accept"], "*/*");
        assert_eq!(request.headers().len(), 1);
    }

    #[test]
    fn decode_request_moves_the_base64_payload_into_the_body() {
        let request = decode_request(
            "POST",
            "ipc://localhost",
            "X-TauriQt-Payload: aGVsbG8=\ncontent-type: application/json\n",
            "ignored",
        )
        .expect("usable request");
        assert_eq!(request.body(), b"hello");
        assert_eq!(request.headers()["content-type"], "application/json");
        assert!(request.headers().get("x-tauriqt-payload").is_none());
    }

    #[test]
    fn decode_request_falls_back_to_the_raw_body_for_an_invalid_payload() {
        let request = decode_request(
            "POST",
            "ipc://localhost",
            "x-tauriqt-payload: !!!not-base64!!!\n",
            "raw-body",
        )
        .expect("usable request");
        assert_eq!(request.body(), b"raw-body");
        assert!(request.headers().get("x-tauriqt-payload").is_none());
    }

    #[test]
    fn decode_request_rejects_an_unusable_request() {
        assert!(decode_request("BOGUS METHOD", "tauri://localhost/", "", "").is_none());
        assert!(decode_request("GET", "not a uri", "", "").is_none());
        assert!(decode_request("GET", "tauri://localhost/", "bad header name: v\n", "").is_none());
    }
}
