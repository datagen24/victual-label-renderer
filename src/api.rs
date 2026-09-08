//! Victual's renderer API, and nothing else.
//!
//! Every destination this module can reach is derived from one base URL given on the command
//! line. There is no field anywhere in a render request that names a host, a port or a URL,
//! which is the property plan 27's security section asks for: the renderer reaches Victual's
//! API and nothing else, and Victual remains the server rather than becoming a client.

use serde_json::Value;

pub struct Api {
    base: String,
    key: String,
    agent: ureq::Agent,
}

#[derive(Debug)]
pub struct ApiError(pub String);

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Api {
    pub fn new(base: &str, key: &str, timeout_seconds: u64) -> Self {
        Api {
            base: base.trim_end_matches('/').to_string(),
            key: key.to_string(),
            agent: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(timeout_seconds))
                .build(),
        }
    }

    /// Claims one render request. `Ok(None)` means the queue was empty, which is the ordinary
    /// answer for a workload that scales to zero and is not an error.
    pub fn claim(&self) -> Result<Option<Value>, ApiError> {
        let body = self.post("/api/labels/render/claim", serde_json::json!({}))?;
        match body.get("render_request_id") {
            Some(Value::Null) | None => Ok(None),
            _ => Ok(Some(body)),
        }
    }

    pub fn asset_bytes(&self, asset_id: u64) -> Result<Vec<u8>, ApiError> {
        let url = format!("{}/api/labels/assets/{}/bytes", self.base, asset_id);
        let response = self
            .agent
            .get(&url)
            .set("VICTUAL-API-KEY", &self.key)
            .call()
            .map_err(|e| ApiError(format!("GET {url}: {e}")))?;
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut response.into_reader(), &mut bytes)
            .map_err(|e| ApiError(format!("reading {url}: {e}")))?;
        Ok(bytes)
    }

    pub fn submit(&self, request_id: u64, token: &str, artifact: &[u8], version: &str) -> Result<Value, ApiError> {
        use base64::Engine;
        self.post(
            &format!("/api/labels/render/{request_id}/result"),
            serde_json::json!({
                "generation_token": token,
                "artifact_base64": base64::engine::general_purpose::STANDARD.encode(artifact),
                "renderer_id": "victual-label-renderer",
                "renderer_version": version,
            }),
        )
    }

    /// An input or layout error, naming the element it is about.
    ///
    /// Reported rather than retried: `invalid` is a document a person has to change, and a
    /// renderer that retried it would loop forever over a label nobody can fix.
    pub fn invalid(&self, request_id: u64, token: &str, code: &str, element: Option<&str>, detail: &str) -> Result<Value, ApiError> {
        self.post(
            &format!("/api/labels/render/{request_id}/invalid"),
            serde_json::json!({ "generation_token": token, "code": code, "element": element, "detail": detail }),
        )
    }

    /// Infrastructure. Retried up to a bound on the server side, because a renderer cannot
    /// touch a printer and so cannot produce a duplicate label by trying again.
    pub fn failed(&self, request_id: u64, token: &str, detail: &str) -> Result<Value, ApiError> {
        self.post(
            &format!("/api/labels/render/{request_id}/failed"),
            serde_json::json!({ "generation_token": token, "detail": detail }),
        )
    }

    fn post(&self, path: &str, body: Value) -> Result<Value, ApiError> {
        let url = format!("{}{path}", self.base);
        let response = self
            .agent
            .post(&url)
            .set("VICTUAL-API-KEY", &self.key)
            .send_json(body);
        match response {
            Ok(ok) => ok.into_json().map_err(|e| ApiError(format!("decoding {url}: {e}"))),
            Err(ureq::Error::Status(code, body)) => {
                let text = body.into_string().unwrap_or_default();
                Err(ApiError(format!("POST {url}: HTTP {code}: {text}")))
            }
            Err(other) => Err(ApiError(format!("POST {url}: {other}"))),
        }
    }
}
