//! [`RegistryApiPort`] over `reqwest` against the loopback Hosted Registry (04 4.3).
//! Plain HTTP only: the registry is bound to `127.0.0.1`, so no TLS stack is needed.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::{ACCEPT, CONTENT_LENGTH, CONTENT_TYPE, LINK};
use reqwest::{Client, Method, Response, StatusCode};
use yanto_core::hosted_registry::MANIFEST_ACCEPT;
use yanto_core::ports::{ManifestInfo, Page, RegistryApiError, RegistryApiPort, RegistryEndpoint};

pub struct HttpRegistryApi {
    client: Client,
}

impl HttpRegistryApi {
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(15))
            .connect_timeout(Duration::from_secs(3))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .expect("reqwest client");
        Self { client }
    }

    fn url(ep: &RegistryEndpoint, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", ep.port)
    }

    async fn send(
        &self,
        ep: &RegistryEndpoint,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        accept: Option<&str>,
    ) -> Result<Response, RegistryApiError> {
        let mut req = self
            .client
            .request(method, Self::url(ep, path))
            .basic_auth(&ep.username, Some(ep.password.expose()))
            .query(query);
        if let Some(a) = accept {
            req = req.header(ACCEPT, a);
        }
        let res = req
            .send()
            .await
            .map_err(|e| RegistryApiError::Unreachable(e.without_url().to_string()))?;
        match res.status() {
            StatusCode::UNAUTHORIZED => Err(RegistryApiError::Unauthorized),
            StatusCode::NOT_FOUND => Err(RegistryApiError::NotFound),
            s if s.is_success() => Ok(res),
            s => {
                let message = res.text().await.unwrap_or_default();
                Err(RegistryApiError::Other {
                    status: s.as_u16(),
                    message: message.chars().take(300).collect(),
                })
            }
        }
    }

    async fn list(
        &self,
        ep: &RegistryEndpoint,
        path: &str,
        key: &str,
        last: Option<&str>,
        n: u32,
    ) -> Result<Page, RegistryApiError> {
        let mut q = vec![("n", n.to_string())];
        if let Some(l) = last {
            q.push(("last", l.to_owned()));
        }
        let res = self.send(ep, Method::GET, path, &q, None).await?;
        let next = res
            .headers()
            .get(LINK)
            .and_then(|v| v.to_str().ok())
            .and_then(next_last);
        let body = res
            .bytes()
            .await
            .map_err(|e| RegistryApiError::Unreachable(e.without_url().to_string()))?;
        let v: serde_json::Value =
            serde_json::from_slice(&body).map_err(|e| RegistryApiError::Other {
                status: 200,
                message: format!("unreadable listing: {e}"),
            })?;
        // `tags` is null for a repository whose tags were all deleted.
        let items = v
            .get(key)
            .and_then(|a| a.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        Ok(Page { items, next })
    }
}

impl Default for HttpRegistryApi {
    fn default() -> Self {
        Self::new()
    }
}

/// The `last` value of a `Link: </v2/...?last=x&n=100>; rel="next"` header.
pub fn next_last(link: &str) -> Option<String> {
    let part = link
        .split(',')
        .find(|p| p.contains("rel=\"next\"") || p.contains("rel=next"))?;
    let target = part.split_once('<')?.1.split_once('>')?.0;
    let url = reqwest::Url::parse(&format!("http://x{target}")).ok()?;
    url.query_pairs()
        .find(|(k, _)| k == "last")
        .map(|(_, v)| v.into_owned())
}

#[async_trait]
impl RegistryApiPort for HttpRegistryApi {
    async fn ping(&self, ep: &RegistryEndpoint) -> Result<(), RegistryApiError> {
        self.send(ep, Method::GET, "/v2/", &[], None)
            .await
            .map(|_| ())
    }

    async fn catalog(
        &self,
        ep: &RegistryEndpoint,
        last: Option<&str>,
        n: u32,
    ) -> Result<Page, RegistryApiError> {
        self.list(ep, "/v2/_catalog", "repositories", last, n).await
    }

    async fn tags(
        &self,
        ep: &RegistryEndpoint,
        repo: &str,
        last: Option<&str>,
        n: u32,
    ) -> Result<Page, RegistryApiError> {
        self.list(ep, &format!("/v2/{repo}/tags/list"), "tags", last, n)
            .await
    }

    async fn manifest(
        &self,
        ep: &RegistryEndpoint,
        repo: &str,
        reference: &str,
        head_only: bool,
    ) -> Result<ManifestInfo, RegistryApiError> {
        let method = if head_only { Method::HEAD } else { Method::GET };
        let res = self
            .send(
                ep,
                method,
                &format!("/v2/{repo}/manifests/{reference}"),
                &[],
                Some(MANIFEST_ACCEPT),
            )
            .await?;
        let h = res.headers();
        let digest = h
            .get("docker-content-digest")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let media_type = h
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned();
        let header_len = h
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let body = if head_only {
            None
        } else {
            Some(
                res.bytes()
                    .await
                    .map_err(|e| RegistryApiError::Unreachable(e.without_url().to_string()))?
                    .to_vec(),
            )
        };
        let manifest_size = header_len
            .or(body.as_ref().map(|b| b.len() as u64))
            .unwrap_or(0);
        Ok(ManifestInfo {
            digest,
            media_type,
            manifest_size,
            body,
        })
    }

    async fn delete_manifest(
        &self,
        ep: &RegistryEndpoint,
        repo: &str,
        digest: &str,
    ) -> Result<(), RegistryApiError> {
        match self
            .send(
                ep,
                Method::DELETE,
                &format!("/v2/{repo}/manifests/{digest}"),
                &[],
                None,
            )
            .await
        {
            Err(RegistryApiError::Other { status: 405, .. }) => {
                Err(RegistryApiError::DeleteDisabled)
            }
            other => other.map(|_| ()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_header_yields_the_next_cursor() {
        let l = r#"</v2/_catalog?last=team%2Fapp&n=100>; rel="next""#;
        assert_eq!(next_last(l).as_deref(), Some("team/app"));
        assert_eq!(next_last("</v2/x?n=1>; rel=\"prev\""), None);
    }
}
