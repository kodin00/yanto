//! In-memory [`RegistryApiPort`]: repositories and tags with real pagination, digest
//! deletion semantics and a scripted reachability state.

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;

use crate::ports::{ManifestInfo, Page, RegistryApiError, RegistryApiPort, RegistryEndpoint};

#[derive(Debug, Default)]
struct State {
    /// repo -> tag -> digest
    repos: BTreeMap<String, BTreeMap<String, String>>,
    /// What `ping` answers; `None` means healthy.
    ping_error: Option<RegistryApiError>,
    delete_disabled: bool,
    /// Calls as `<verb> <what>` for assertions.
    calls: Vec<String>,
    /// Password the fake accepts for the internal user (None = anything).
    expect_password: Option<String>,
    /// Layer sizes by digest, for the manifest body.
    sizes: BTreeMap<String, u64>,
}

#[derive(Debug, Default)]
pub struct FakeRegistryApi(Mutex<State>);

pub fn fake_digest(seed: &str) -> String {
    format!("sha256:{}", crate::crypto::sha256_hex(seed))
}

impl FakeRegistryApi {
    /// Add (or move) a tag to a manifest identified by `digest`.
    pub fn push(&self, repo: &str, tag: &str, digest: &str, layer_bytes: u64) {
        let mut s = self.0.lock().expect("state");
        s.repos
            .entry(repo.to_owned())
            .or_default()
            .insert(tag.to_owned(), digest.to_owned());
        s.sizes.insert(digest.to_owned(), layer_bytes);
    }

    pub fn tags_of(&self, repo: &str) -> Vec<String> {
        self.0
            .lock()
            .expect("state")
            .repos
            .get(repo)
            .map(|t| t.keys().cloned().collect())
            .unwrap_or_default()
    }

    pub fn set_ping_error(&self, e: Option<RegistryApiError>) {
        self.0.lock().expect("state").ping_error = e;
    }

    pub fn set_delete_disabled(&self, v: bool) {
        self.0.lock().expect("state").delete_disabled = v;
    }

    pub fn expect_password(&self, p: Option<&str>) {
        self.0.lock().expect("state").expect_password = p.map(str::to_owned);
    }

    pub fn calls(&self) -> Vec<String> {
        self.0.lock().expect("state").calls.clone()
    }

    fn check(&self, ep: &RegistryEndpoint, what: String) -> Result<(), RegistryApiError> {
        let mut s = self.0.lock().expect("state");
        s.calls.push(what);
        if let Some(e) = &s.ping_error {
            return Err(e.clone());
        }
        if let Some(p) = &s.expect_password
            && p != ep.password.expose()
        {
            return Err(RegistryApiError::Unauthorized);
        }
        Ok(())
    }
}

fn page(mut all: Vec<String>, last: Option<&str>, n: u32) -> Page {
    all.sort();
    if let Some(l) = last {
        all.retain(|x| x.as_str() > l);
    }
    let n = n.max(1) as usize;
    let next = (all.len() > n).then(|| all[n - 1].clone());
    all.truncate(n);
    Page { items: all, next }
}

#[async_trait]
impl RegistryApiPort for FakeRegistryApi {
    async fn ping(&self, ep: &RegistryEndpoint) -> Result<(), RegistryApiError> {
        self.check(ep, "ping".into())
    }

    async fn catalog(
        &self,
        ep: &RegistryEndpoint,
        last: Option<&str>,
        n: u32,
    ) -> Result<Page, RegistryApiError> {
        self.check(ep, "catalog".into())?;
        let all = self
            .0
            .lock()
            .expect("state")
            .repos
            .keys()
            .cloned()
            .collect();
        Ok(page(all, last, n))
    }

    async fn tags(
        &self,
        ep: &RegistryEndpoint,
        repo: &str,
        last: Option<&str>,
        n: u32,
    ) -> Result<Page, RegistryApiError> {
        self.check(ep, format!("tags {repo}"))?;
        let all = self
            .0
            .lock()
            .expect("state")
            .repos
            .get(repo)
            .ok_or(RegistryApiError::NotFound)?
            .keys()
            .cloned()
            .collect();
        Ok(page(all, last, n))
    }

    async fn manifest(
        &self,
        ep: &RegistryEndpoint,
        repo: &str,
        reference: &str,
        head_only: bool,
    ) -> Result<ManifestInfo, RegistryApiError> {
        self.check(
            ep,
            format!(
                "{} {repo}:{reference}",
                if head_only { "head" } else { "get" }
            ),
        )?;
        let s = self.0.lock().expect("state");
        let tags = s.repos.get(repo).ok_or(RegistryApiError::NotFound)?;
        let digest = if reference.starts_with("sha256:") {
            tags.values()
                .find(|d| d.as_str() == reference)
                .cloned()
                .ok_or(RegistryApiError::NotFound)?
        } else {
            tags.get(reference)
                .cloned()
                .ok_or(RegistryApiError::NotFound)?
        };
        let size = s.sizes.get(&digest).copied().unwrap_or(0);
        let body = format!(
            r#"{{"schemaVersion":2,"mediaType":"application/vnd.docker.distribution.manifest.v2+json","config":{{"size":100,"digest":"sha256:cfg"}},"layers":[{{"size":{size},"digest":"sha256:layer"}}]}}"#
        )
        .into_bytes();
        Ok(ManifestInfo {
            digest,
            media_type: "application/vnd.docker.distribution.manifest.v2+json".into(),
            manifest_size: body.len() as u64,
            body: (!head_only).then_some(body),
        })
    }

    async fn delete_manifest(
        &self,
        ep: &RegistryEndpoint,
        repo: &str,
        digest: &str,
    ) -> Result<(), RegistryApiError> {
        self.check(ep, format!("delete {repo}@{digest}"))?;
        let mut s = self.0.lock().expect("state");
        if s.delete_disabled {
            return Err(RegistryApiError::DeleteDisabled);
        }
        let tags = s.repos.get_mut(repo).ok_or(RegistryApiError::NotFound)?;
        let before = tags.len();
        tags.retain(|_, d| d != digest);
        if tags.len() == before {
            return Err(RegistryApiError::NotFound);
        }
        Ok(())
    }
}
