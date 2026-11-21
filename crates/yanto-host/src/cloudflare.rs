//! [`CloudflarePort`] over the Cloudflare v4 REST API with `reqwest` (rustls). The base URL is a
//! constructor argument so tests point it at a mock server. Paths are the ones in 04 9.2; response
//! envelopes are `{success, errors[], result}`. The API token is only ever sent as a bearer header
//! and never appears in an error message.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::{Client, Method, RequestBuilder, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use yanto_core::ports::{
    CfAuth, CfDnsRecord, CfError, CfResult, CfTunnel, CfZone, CloudflarePort, DnsRecordInput,
    IngressRule,
};
use yanto_core::secrets::Secret;

pub const DEFAULT_BASE: &str = "https://api.cloudflare.com/client/v4";
/// 429 and 5xx are retried with backoff (04 9.2: 3 tries).
const TRIES: u32 = 3;

pub struct CloudflareHttp {
    client: Client,
    base: String,
    backoff: Duration,
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    errors: Vec<ApiError>,
    #[serde(default)]
    result: Value,
}

#[derive(Deserialize)]
struct ApiError {
    #[serde(default)]
    message: String,
}

impl CloudflareHttp {
    pub fn new(base: &str) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(20))
            .user_agent("yanto")
            .build()
            .expect("reqwest client");
        Self {
            client,
            base: base.trim_end_matches('/').to_owned(),
            backoff: Duration::from_millis(500),
        }
    }

    /// Shorter retry delay for tests.
    pub fn with_backoff(mut self, d: Duration) -> Self {
        self.backoff = d;
        self
    }

    fn req(&self, m: Method, path: &str, token: &Secret) -> RequestBuilder {
        self.client
            .request(m, format!("{}{}", self.base, path))
            .bearer_auth(token.expose())
    }

    /// Send and unwrap the envelope; retry 429/5xx up to [`TRIES`] times.
    async fn call(&self, build: impl Fn() -> RequestBuilder) -> CfResult<Value> {
        let mut last = CfError {
            status: 0,
            message: "no attempt made".into(),
        };
        for attempt in 0..TRIES {
            if attempt > 0 {
                tokio::time::sleep(self.backoff * 2u32.pow(attempt - 1)).await;
            }
            let resp = match build().send().await {
                Ok(r) => r,
                Err(e) => {
                    // The URL never contains the token; the error text is safe.
                    last = CfError {
                        status: 0,
                        message: e.without_url().to_string(),
                    };
                    continue;
                }
            };
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            let env: Option<Envelope> = serde_json::from_str(&body).ok();
            if status.is_success() && env.as_ref().is_none_or(|e| e.success) {
                return Ok(env.map_or(Value::Null, |e| e.result));
            }
            let message = env
                .as_ref()
                .and_then(|e| e.errors.first())
                .map(|e| e.message.clone())
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| status.canonical_reason().unwrap_or("error").to_owned());
            last = CfError {
                status: status.as_u16(),
                message,
            };
            if status != StatusCode::TOO_MANY_REQUESTS && !status.is_server_error() {
                break;
            }
        }
        Err(last)
    }
}

fn s(v: &Value, k: &str) -> String {
    v.get(k)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn tunnel(v: &Value) -> CfTunnel {
    CfTunnel {
        id: s(v, "id"),
        name: s(v, "name"),
        status: v
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned(),
    }
}

fn record(v: &Value) -> CfDnsRecord {
    CfDnsRecord {
        id: s(v, "id"),
        record_type: s(v, "type"),
        name: s(v, "name").to_ascii_lowercase(),
        content: s(v, "content"),
        ttl: v.get("ttl").and_then(Value::as_i64).unwrap_or(1),
        proxied: v.get("proxied").and_then(Value::as_bool).unwrap_or(false),
        priority: v.get("priority").and_then(Value::as_i64),
        comment: v
            .get("comment")
            .and_then(Value::as_str)
            .filter(|c| !c.is_empty())
            .map(str::to_owned),
    }
}

fn dns_body(r: &DnsRecordInput) -> Value {
    let mut b = json!({
        "type": r.record_type, "name": r.name, "content": r.content,
        "ttl": r.ttl, "proxied": r.proxied,
    });
    if let Some(p) = r.priority {
        b["priority"] = json!(p);
    }
    if let Some(c) = &r.comment {
        b["comment"] = json!(c);
    }
    b
}

fn rules_json(rules: &[IngressRule]) -> Value {
    let list: Vec<Value> = rules
        .iter()
        .map(|r| match &r.hostname {
            Some(h) => json!({ "hostname": h, "service": r.service }),
            None => json!({ "service": r.service }),
        })
        .collect();
    json!({ "config": { "ingress": list } })
}

#[async_trait]
impl CloudflarePort for CloudflareHttp {
    async fn check_tunnels(&self, a: CfAuth<'_>) -> CfResult<()> {
        let path = format!("/accounts/{}/cfd_tunnel?per_page=1", a.account_id);
        self.call(|| self.req(Method::GET, &path, a.token))
            .await
            .map(|_| ())
    }

    async fn list_zones(&self, a: CfAuth<'_>) -> CfResult<Vec<CfZone>> {
        let path = format!("/zones?account.id={}&per_page=50", a.account_id);
        let v = self.call(|| self.req(Method::GET, &path, a.token)).await?;
        Ok(v.as_array()
            .map(|l| {
                l.iter()
                    .map(|z| CfZone {
                        id: s(z, "id"),
                        name: s(z, "name").to_ascii_lowercase(),
                        status: z.get("status").and_then(Value::as_str).map(str::to_owned),
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn create_tunnel(&self, a: CfAuth<'_>, name: &str) -> CfResult<CfTunnel> {
        let path = format!("/accounts/{}/cfd_tunnel", a.account_id);
        let body = json!({ "name": name, "config_src": "cloudflare" });
        let v = self
            .call(|| self.req(Method::POST, &path, a.token).json(&body))
            .await?;
        Ok(tunnel(&v))
    }

    async fn get_tunnel(&self, a: CfAuth<'_>, id: &str) -> CfResult<CfTunnel> {
        let path = format!("/accounts/{}/cfd_tunnel/{id}", a.account_id);
        let v = self.call(|| self.req(Method::GET, &path, a.token)).await?;
        Ok(tunnel(&v))
    }

    async fn tunnel_token(&self, a: CfAuth<'_>, id: &str) -> CfResult<Secret> {
        let path = format!("/accounts/{}/cfd_tunnel/{id}/token", a.account_id);
        let v = self.call(|| self.req(Method::GET, &path, a.token)).await?;
        match v.as_str() {
            Some(t) if !t.is_empty() => Ok(Secret::new(t)),
            _ => Err(CfError {
                status: 200,
                message: "the API returned no tunnel token".into(),
            }),
        }
    }

    async fn put_ingress(&self, a: CfAuth<'_>, id: &str, rules: &[IngressRule]) -> CfResult<()> {
        let path = format!("/accounts/{}/cfd_tunnel/{id}/configurations", a.account_id);
        let body = rules_json(rules);
        self.call(|| self.req(Method::PUT, &path, a.token).json(&body))
            .await
            .map(|_| ())
    }

    async fn get_ingress(&self, a: CfAuth<'_>, id: &str) -> CfResult<Vec<IngressRule>> {
        let path = format!("/accounts/{}/cfd_tunnel/{id}/configurations", a.account_id);
        let v = self.call(|| self.req(Method::GET, &path, a.token)).await?;
        Ok(v.pointer("/config/ingress")
            .and_then(Value::as_array)
            .map(|l| {
                l.iter()
                    .map(|r| IngressRule {
                        hostname: r.get("hostname").and_then(Value::as_str).map(str::to_owned),
                        service: s(r, "service"),
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn delete_tunnel(&self, a: CfAuth<'_>, id: &str) -> CfResult<()> {
        let path = format!("/accounts/{}/cfd_tunnel/{id}", a.account_id);
        self.call(|| self.req(Method::DELETE, &path, a.token))
            .await
            .map(|_| ())
    }

    async fn list_dns(
        &self,
        token: &Secret,
        zone: &str,
        ty: Option<&str>,
        name: Option<&str>,
    ) -> CfResult<Vec<CfDnsRecord>> {
        let mut q: Vec<(&str, String)> = vec![("per_page", "200".into())];
        if let Some(t) = ty {
            q.push(("type", t.into()));
        }
        if let Some(n) = name {
            q.push(("name", n.into()));
        }
        let path = format!("/zones/{zone}/dns_records");
        let v = self
            .call(|| self.req(Method::GET, &path, token).query(&q))
            .await?;
        Ok(v.as_array()
            .map(|l| l.iter().map(record).collect())
            .unwrap_or_default())
    }

    async fn create_dns(
        &self,
        token: &Secret,
        zone: &str,
        rec: &DnsRecordInput,
    ) -> CfResult<CfDnsRecord> {
        let path = format!("/zones/{zone}/dns_records");
        let body = dns_body(rec);
        let v = self
            .call(|| self.req(Method::POST, &path, token).json(&body))
            .await?;
        Ok(record(&v))
    }

    async fn update_dns(
        &self,
        token: &Secret,
        zone: &str,
        id: &str,
        rec: &DnsRecordInput,
    ) -> CfResult<CfDnsRecord> {
        let path = format!("/zones/{zone}/dns_records/{id}");
        let body = dns_body(rec);
        let v = self
            .call(|| self.req(Method::PUT, &path, token).json(&body))
            .await?;
        Ok(record(&v))
    }

    async fn delete_dns(&self, token: &Secret, zone: &str, id: &str) -> CfResult<()> {
        let path = format!("/zones/{zone}/dns_records/{id}");
        self.call(|| self.req(Method::DELETE, &path, token))
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_json, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ACC: &str = "acc123";

    fn api(server: &MockServer) -> CloudflareHttp {
        CloudflareHttp::new(&server.uri()).with_backoff(Duration::from_millis(1))
    }

    fn ok(result: Value) -> ResponseTemplate {
        ResponseTemplate::new(200)
            .set_body_json(json!({"success": true, "errors": [], "result": result}))
    }

    fn fail(status: u16, message: &str) -> ResponseTemplate {
        ResponseTemplate::new(status).set_body_json(
            json!({"success": false, "errors": [{"code": 1, "message": message}], "result": null}),
        )
    }

    #[tokio::test]
    async fn validate_calls_and_bearer_header() {
        let server = MockServer::start().await;
        let tok = Secret::new("tok-1");
        Mock::given(method("GET"))
            .and(path("/accounts/acc123/cfd_tunnel"))
            .and(query_param("per_page", "1"))
            .and(header("authorization", "Bearer tok-1"))
            .respond_with(ok(json!([])))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/zones"))
            .and(query_param("account.id", ACC))
            .and(query_param("per_page", "50"))
            .respond_with(ok(
                json!([{"id":"z1","name":"Example.com","status":"active"}]),
            ))
            .expect(1)
            .mount(&server)
            .await;
        let a = CfAuth {
            account_id: ACC,
            token: &tok,
        };
        let c = api(&server);
        c.check_tunnels(a).await.unwrap();
        let z = c.list_zones(a).await.unwrap();
        assert_eq!(z[0].name, "example.com");
    }

    #[tokio::test]
    async fn bad_token_error_names_status_and_message_but_not_the_token() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(fail(403, "Invalid access token"))
            .mount(&server)
            .await;
        let tok = Secret::new("super-secret-token");
        let e = api(&server)
            .check_tunnels(CfAuth {
                account_id: ACC,
                token: &tok,
            })
            .await
            .unwrap_err();
        assert_eq!(e.status, 403);
        assert!(e.message.contains("Invalid access token"));
        assert!(!e.to_string().contains("super-secret-token"));
    }

    #[tokio::test]
    async fn tunnel_create_ingress_token_and_delete() {
        let server = MockServer::start().await;
        let tok = Secret::new("t");
        let ingress = json!([
            {"hostname": "a.example.com", "service": "http://127.0.0.1:8080"},
            {"service": "http_status:404"}
        ]);
        Mock::given(method("POST"))
            .and(path("/accounts/acc123/cfd_tunnel"))
            .and(body_json(
                json!({"name": "vps", "config_src": "cloudflare"}),
            ))
            .respond_with(ok(
                json!({"id": "uuid-1", "name": "vps", "status": "inactive"}),
            ))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/accounts/acc123/cfd_tunnel/uuid-1/configurations"))
            .and(body_json(json!({"config": {"ingress": ingress.clone()}})))
            .respond_with(ok(json!({})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/accounts/acc123/cfd_tunnel/uuid-1/configurations"))
            .respond_with(ok(json!({"config": {"ingress": ingress}})))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/accounts/acc123/cfd_tunnel/uuid-1/token"))
            .respond_with(ok(json!("connector-token-value")))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/accounts/acc123/cfd_tunnel/uuid-1"))
            .respond_with(ok(json!({"id": "uuid-1"})))
            .expect(1)
            .mount(&server)
            .await;
        let a = CfAuth {
            account_id: ACC,
            token: &tok,
        };
        let c = api(&server);
        let t = c.create_tunnel(a, "vps").await.unwrap();
        assert_eq!((t.id.as_str(), t.status.as_str()), ("uuid-1", "inactive"));
        let rules = vec![
            IngressRule {
                hostname: Some("a.example.com".into()),
                service: "http://127.0.0.1:8080".into(),
            },
            IngressRule {
                hostname: None,
                service: "http_status:404".into(),
            },
        ];
        c.put_ingress(a, "uuid-1", &rules).await.unwrap();
        assert_eq!(c.get_ingress(a, "uuid-1").await.unwrap(), rules);
        assert_eq!(
            c.tunnel_token(a, "uuid-1").await.unwrap().expose(),
            "connector-token-value"
        );
        c.delete_tunnel(a, "uuid-1").await.unwrap();
    }

    #[tokio::test]
    async fn dns_crud_shapes() {
        let server = MockServer::start().await;
        let tok = Secret::new("t");
        let rec = json!({"id": "r1", "type": "CNAME", "name": "App.Example.com", "content": "uuid.cfargotunnel.com", "ttl": 1, "proxied": true, "comment": "managed-by=yanto"});
        Mock::given(method("GET"))
            .and(path("/zones/z1/dns_records"))
            .and(query_param("type", "CNAME"))
            .and(query_param("name", "app.example.com"))
            .and(query_param("per_page", "200"))
            .respond_with(ok(json!([rec.clone()])))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/zones/z1/dns_records"))
            .and(body_json(json!({"type":"CNAME","name":"app.example.com","content":"uuid.cfargotunnel.com","ttl":1,"proxied":true,"comment":"managed-by=yanto"})))
            .respond_with(ok(rec.clone()))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/zones/z1/dns_records/r1"))
            .respond_with(ok(rec.clone()))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/zones/z1/dns_records/r1"))
            .respond_with(ok(json!({"id":"r1"})))
            .expect(1)
            .mount(&server)
            .await;
        let c = api(&server);
        let input = DnsRecordInput {
            record_type: "CNAME".into(),
            name: "app.example.com".into(),
            content: "uuid.cfargotunnel.com".into(),
            ttl: 1,
            proxied: true,
            priority: None,
            comment: Some("managed-by=yanto".into()),
        };
        let l = c
            .list_dns(&tok, "z1", Some("CNAME"), Some("app.example.com"))
            .await
            .unwrap();
        assert_eq!(l[0].name, "app.example.com");
        assert!(l[0].proxied && l[0].comment.as_deref() == Some("managed-by=yanto"));
        c.create_dns(&tok, "z1", &input).await.unwrap();
        c.update_dns(&tok, "z1", "r1", &input).await.unwrap();
        c.delete_dns(&tok, "z1", "r1").await.unwrap();
    }

    #[tokio::test]
    async fn rate_limit_is_retried_then_succeeds_or_errors() {
        let server = MockServer::start().await;
        let tok = Secret::new("t");
        let a = CfAuth {
            account_id: ACC,
            token: &tok,
        };
        Mock::given(method("GET"))
            .and(path("/accounts/acc123/cfd_tunnel"))
            .respond_with(fail(429, "rate limited"))
            .up_to_n_times(2)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/accounts/acc123/cfd_tunnel"))
            .respond_with(ok(json!([])))
            .mount(&server)
            .await;
        api(&server).check_tunnels(a).await.unwrap();

        let always = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(fail(429, "rate limited"))
            .expect(3)
            .mount(&always)
            .await;
        let e = api(&always).check_tunnels(a).await.unwrap_err();
        assert_eq!(e.status, 429);
    }

    #[tokio::test]
    async fn unreachable_api_is_a_status_zero_error() {
        let c = CloudflareHttp::new("http://127.0.0.1:1").with_backoff(Duration::from_millis(1));
        let tok = Secret::new("t");
        let e = c
            .check_tunnels(CfAuth {
                account_id: ACC,
                token: &tok,
            })
            .await
            .unwrap_err();
        assert_eq!(e.status, 0);
        assert!(e.to_string().contains("unreachable"));
    }
}
