//! [`IngressProbePort`]: local TCP/HTTP probes of a target port and the public `HEAD` check.
//! Probes report status codes only; response bodies are never read.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use reqwest::Client;
use tokio::net::TcpStream;
use yanto_core::ports::IngressProbePort;

pub struct HostIngressProbe {
    /// Where "loopback" is: `127.0.0.1`, overridable for tests.
    local_host: String,
}

impl Default for HostIngressProbe {
    fn default() -> Self {
        Self {
            local_host: "127.0.0.1".into(),
        }
    }
}

fn client(timeout: Duration) -> Client {
    Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("reqwest client")
}

impl HostIngressProbe {
    async fn get_status(&self, port: u16, host: &str, timeout: Duration) -> Result<u16, String> {
        client(timeout)
            .get(format!("http://{}:{port}/", self.local_host))
            .header(reqwest::header::HOST, host)
            .send()
            .await
            .map(|r| r.status().as_u16())
            .map_err(|e| e.without_url().to_string())
    }
}

#[async_trait]
impl IngressProbePort for HostIngressProbe {
    async fn tcp_connect(&self, port: u16) -> bool {
        tokio::time::timeout(
            Duration::from_secs(1),
            TcpStream::connect((self.local_host.as_str(), port)),
        )
        .await
        .is_ok_and(|r| r.is_ok())
    }

    async fn local_http(&self, port: u16, host: &str, timeout_secs: u64) -> Result<u16, String> {
        self.get_status(port, host, Duration::from_secs(timeout_secs.max(1)))
            .await
    }

    async fn await_route(&self, port: u16, host: &str, wait_secs: u64) -> Result<u16, String> {
        let deadline = Instant::now() + Duration::from_secs(wait_secs);
        loop {
            let r = self.get_status(port, host, Duration::from_secs(3)).await;
            match &r {
                Ok(s) if *s != 404 => return r,
                _ if Instant::now() >= deadline => return r,
                _ => tokio::time::sleep(Duration::from_millis(500)).await,
            }
        }
    }

    async fn public_head(&self, hostname: &str) -> Result<u16, String> {
        client(Duration::from_secs(5))
            .head(format!("https://{hostname}/"))
            .send()
            .await
            .map(|r| r.status().as_u16())
            .map_err(|e| e.without_url().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn local_probe_sends_the_host_header_and_reports_status_only() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(header("host", "app.example.com"))
            .respond_with(ResponseTemplate::new(204).set_body_string("body is ignored"))
            .expect(2)
            .mount(&server)
            .await;
        let port = server.address().port();
        let p = HostIngressProbe::default();
        assert!(p.tcp_connect(port).await);
        assert_eq!(p.local_http(port, "app.example.com", 3).await, Ok(204));
        assert_eq!(p.await_route(port, "app.example.com", 1).await, Ok(204));
        drop(server);
        assert!(p.local_http(1, "x", 1).await.is_err());
        assert!(!p.tcp_connect(1).await);
    }

    #[tokio::test]
    async fn await_route_returns_the_last_404_after_the_wait() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let p = HostIngressProbe::default();
        let r = p
            .await_route(server.address().port(), "x.example.com", 1)
            .await;
        assert_eq!(r, Ok(404));
    }
}
