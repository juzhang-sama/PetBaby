use base64::Engine;
use reqwest::StatusCode;
use std::time::Duration;

const DEFAULT_BASE: &str = "https://api.lk888.ai";
const DEFAULT_MODEL: &str = "gpt-image-2";

#[derive(Debug, thiserror::Error)]
pub enum GenError {
    #[error("network error: {0}")]
    Network(String),
    #[error("auth error: {0}")]
    Auth(String),
    #[error("rate limited: {0}")]
    RateLimit(String),
    #[error("generation error: {0}")]
    Generation(String),
    #[expect(dead_code)] // reserved for the polling loop
    #[error("timeout: {0}")]
    Timeout(String),
}

#[derive(Debug, Clone)]
#[expect(dead_code)] // task_id kept for diagnostics in the polling loop
pub struct TaskState {
    pub task_id: String,
    pub state: String,
    pub is_final: bool,
    pub result_url: Option<String>,
    pub error: Option<String>,
}

pub struct Lk888Client {
    key: String,
    base: String,
    model: String,
    client: reqwest::Client,
}

impl Lk888Client {
    pub fn new(key: String) -> Self {
        Self {
            key,
            base: std::env::var("LK888_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE.to_string()),
            model: std::env::var("LK888_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string()),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(60))
                .build()
                .expect("http client"),
        }
    }

    #[expect(dead_code)] // used by unit tests with a mock server
    pub fn new_with(key: String, base: String, model: String) -> Self {
        Self {
            key,
            base,
            model,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(60))
                .build()
                .expect("http client"),
        }
    }

    fn headers(&self) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        let auth = format!("Bearer {}", self.key);
        headers.insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_str(&auth).expect("auth header"),
        );
        headers
    }

    fn data_url(png: &[u8]) -> String {
        format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(png)
        )
    }

    pub async fn submit(
        &self,
        prompt: &str,
        ref_image_png: Option<&[u8]>,
        size: &str,
    ) -> Result<String, GenError> {
        let mut params = serde_json::json!({
            "size": size,
            "quality": "auto",
            "n": 1,
            "response_format": "url",
        });
        if let Some(png) = ref_image_png {
            params["images"] = serde_json::json!([Self::data_url(png)]);
        }
        let body = serde_json::json!({
            "model": self.model,
            "prompt": prompt,
            "params": params,
        });

        let mut last_error: Option<GenError> = None;
        for attempt in 1..=3 {
            let result = self
                .client
                .post(format!("{}/v1/media/generate", self.base))
                .headers(self.headers())
                .json(&body)
                .send()
                .await;
            match result {
                Ok(response) => {
                    let status = response.status();
                    let text = response.text().await.unwrap_or_default();
                    if status.is_success() {
                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                            if let Some(task_id) = parsed.get("data").and_then(|d| d.get("task_id"))
                            {
                                return Ok(task_id.as_str().unwrap_or_default().to_string());
                            }
                        }
                        last_error = Some(GenError::Generation(format!(
                            "no task_id: {}",
                            text.chars().take(300).collect::<String>()
                        )));
                    } else if status == StatusCode::UNAUTHORIZED {
                        return Err(GenError::Auth(text.chars().take(200).collect()));
                    } else if status == StatusCode::TOO_MANY_REQUESTS {
                        return Err(GenError::RateLimit(text.chars().take(200).collect()));
                    } else {
                        last_error = Some(GenError::Generation(format!(
                            "submit {}: {}",
                            status,
                            text.chars().take(300).collect::<String>()
                        )));
                    }
                }
                Err(error) => {
                    last_error = Some(GenError::Network(error.to_string()));
                }
            }
            if attempt < 3 {
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
        Err(last_error.unwrap_or_else(|| GenError::Network("submit failed".into())))
    }

    pub async fn poll(&self, task_id: &str) -> Result<TaskState, GenError> {
        let response = self
            .client
            .get(format!("{}/v1/media/status", self.base))
            .headers(self.headers())
            .query(&[("task_id", task_id)])
            .send()
            .await
            .map_err(|error| GenError::Network(error.to_string()))?;
        let text = response
            .text()
            .await
            .map_err(|error| GenError::Network(error.to_string()))?;
        let parsed: serde_json::Value = serde_json::from_str(&text)
            .map_err(|error| GenError::Generation(format!("bad poll response: {error}")))?;
        Ok(TaskState {
            task_id: task_id.to_string(),
            state: parsed
                .get("state")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string(),
            is_final: parsed
                .get("is_final")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            result_url: parsed
                .get("result_url")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            error: parsed
                .get("error")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        })
    }

    pub async fn download(&self, url: &str) -> Result<Vec<u8>, GenError> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|error| GenError::Network(error.to_string()))?;
        if !response.status().is_success() {
            return Err(GenError::Network(format!("download {}", response.status())));
        }
        response
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|error| GenError::Network(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use wiremock::{
        matchers::{method, path, query_param},
        Mock, MockServer, ResponseTemplate,
    };

    use super::Lk888Client;

    #[tokio::test]
    async fn submit_sends_expected_payload() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/media/generate"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 200,
                "data": { "task_id": "task-1" }
            })))
            .mount(&server)
            .await;

        let client = Lk888Client::new_with("k".into(), server.uri(), "gpt-image-2".into());
        let png = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        let task = client.submit("a cat", Some(&png), "auto").await.unwrap();
        assert_eq!(task, "task-1");

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        let body: serde_json::Value = serde_json::from_slice(&received[0].body).expect("json body");
        assert_eq!(body["model"], "gpt-image-2");
        assert_eq!(body["prompt"], "a cat");
        assert_eq!(body["params"]["size"], "auto");
        let expected_url = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(png)
        );
        assert_eq!(body["params"]["images"][0], expected_url);
    }

    #[tokio::test]
    async fn submit_retries_on_transient_failure() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/media/generate"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&server)
            .await;

        let client = Lk888Client::new_with("k".into(), server.uri(), "m".into());
        let result = client.submit("a cat", None, "auto").await;
        assert!(result.is_err());
        let received = server.received_requests().await.unwrap();
        assert!(received.len() >= 2, "expected retries");
    }

    #[tokio::test]
    async fn poll_maps_success_state() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/media/status"))
            .and(query_param("task_id", "t1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "task_id": "t1",
                "state": "success",
                "is_final": true,
                "result_url": "https://x.test/out.png"
            })))
            .mount(&server)
            .await;

        let client = Lk888Client::new_with("k".into(), server.uri(), "m".into());
        let state = client.poll("t1").await.unwrap();
        assert!(state.is_final);
        assert_eq!(state.state, "success");
        assert_eq!(state.result_url.as_deref(), Some("https://x.test/out.png"));
    }

    #[tokio::test]
    async fn poll_maps_running_state() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/media/status"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "task_id": "t1",
                "state": "running",
                "is_final": false
            })))
            .mount(&server)
            .await;

        let client = Lk888Client::new_with("k".into(), server.uri(), "m".into());
        let state = client.poll("t1").await.unwrap();
        assert!(!state.is_final);
    }
}
