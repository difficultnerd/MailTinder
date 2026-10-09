//! A fake `HttpEgress` that refuses everything not explicitly routed.

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::Mutex;

use async_trait::async_trait;
use ports::{EgressError, EgressRequest, EgressResponse, HttpEgress, HttpMethod, OneClickOutcome};
use url::Url;

/// How a routed host is handled.
#[derive(Clone)]
pub enum Route {
    Forward(SocketAddr),
    Scripted(VecDeque<Result<EgressResponse, EgressError>>),
}

/// A recorded egress call.
#[derive(Clone, Debug)]
pub struct EgressRecord {
    pub host: String,
    pub method: HttpMethod,
    pub path: String,
    pub header_names: Vec<String>,
    pub allowed: bool,
}

/// A fake egress that refuses every host by default.
pub struct FakeHttpEgress {
    routes: Mutex<HashMap<String, Route>>,
    one_click: Mutex<VecDeque<Result<OneClickOutcome, EgressError>>>,
    log: Mutex<Vec<EgressRecord>>,
}

impl FakeHttpEgress {
    pub fn new() -> Self {
        Self {
            routes: Mutex::new(HashMap::new()),
            one_click: Mutex::new(VecDeque::new()),
            log: Mutex::new(Vec::new()),
        }
    }

    pub fn forward(&self, host: &str, to: SocketAddr) {
        self.routes
            .lock()
            .unwrap_or_else(|_| panic!("egress poisoned"))
            .insert(host.to_owned(), Route::Forward(to));
    }

    pub fn script(&self, host: &str, r: Result<EgressResponse, EgressError>) {
        let mut routes = self
            .routes
            .lock()
            .unwrap_or_else(|_| panic!("egress poisoned"));
        let entry = routes
            .entry(host.to_owned())
            .or_insert_with(|| Route::Scripted(VecDeque::new()));
        if let Route::Scripted(q) = entry {
            q.push_back(r);
        }
    }

    pub fn script_one_click(&self, r: Result<OneClickOutcome, EgressError>) {
        self.one_click
            .lock()
            .unwrap_or_else(|_| panic!("egress poisoned"))
            .push_back(r);
    }

    pub fn records(&self) -> Vec<EgressRecord> {
        self.log
            .lock()
            .unwrap_or_else(|_| panic!("egress poisoned"))
            .clone()
    }

    pub fn violations(&self) -> Vec<EgressRecord> {
        self.records().into_iter().filter(|r| !r.allowed).collect()
    }

    fn record(
        &self,
        host: &str,
        method: HttpMethod,
        path: &str,
        header_names: Vec<String>,
        allowed: bool,
    ) {
        self.log
            .lock()
            .unwrap_or_else(|_| panic!("egress poisoned"))
            .push(EgressRecord {
                host: host.to_owned(),
                method,
                path: path.to_owned(),
                header_names,
                allowed,
            });
    }
}

impl Default for FakeHttpEgress {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HttpEgress for FakeHttpEgress {
    async fn one_click_post(&self, url: &Url) -> Result<OneClickOutcome, EgressError> {
        let host = url.host_str().unwrap_or_default().to_owned();
        let route = self
            .routes
            .lock()
            .unwrap_or_else(|_| panic!("egress poisoned"))
            .get(&host)
            .cloned();
        match route {
            Some(Route::Forward(addr)) => {
                let client = reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .no_proxy()
                    .build()
                    .map_err(|_| EgressError::Connect)?;
                let target = format!("http://{addr}{}", url.path());
                let resp = client
                    .post(&target)
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body("List-Unsubscribe=One-Click")
                    .send()
                    .await
                    .map_err(|_| EgressError::Connect)?;
                let status = resp.status().as_u16();
                Ok(match status {
                    200..=299 => OneClickOutcome::Accepted { status },
                    300..=399 => OneClickOutcome::Redirected { status },
                    _ => OneClickOutcome::Rejected { status },
                })
            }
            _ => {
                let mut q = self
                    .one_click
                    .lock()
                    .unwrap_or_else(|_| panic!("egress poisoned"));
                match q.pop_front() {
                    Some(r) => r,
                    None => {
                        self.record(&host, HttpMethod::Post, url.path(), vec![], false);
                        Err(EgressError::HostNotAllowed)
                    }
                }
            }
        }
    }

    async fn call(&self, req: EgressRequest) -> Result<EgressResponse, EgressError> {
        let host = req.url.host_str().unwrap_or_default().to_owned();
        let header_names = req.headers.iter().map(|(k, _)| k.clone()).collect();
        let route = self
            .routes
            .lock()
            .unwrap_or_else(|_| panic!("egress poisoned"))
            .get(&host)
            .cloned();
        match route {
            Some(Route::Forward(addr)) => {
                let client = reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .no_proxy()
                    .timeout(req.timeout)
                    .build()
                    .map_err(|_| EgressError::Connect)?;
                let target = format!("http://{addr}{}", req.url.path());
                let mut rb = match req.method {
                    HttpMethod::Get => client.get(&target),
                    HttpMethod::Post => client.post(&target),
                    HttpMethod::Put => client.put(&target),
                    HttpMethod::Patch => client.patch(&target),
                    HttpMethod::Delete => client.delete(&target),
                };
                for (k, v) in &req.headers {
                    rb = rb.header(k, v.expose());
                }
                if let Some(body) = &req.body {
                    rb = rb.body(body.clone());
                }
                let resp = rb.send().await.map_err(|e| {
                    if e.is_timeout() {
                        EgressError::Timeout
                    } else {
                        EgressError::Connect
                    }
                })?;
                let status = resp.status().as_u16();
                let headers = resp
                    .headers()
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or_default().to_owned()))
                    .collect();
                let body = resp
                    .bytes()
                    .await
                    .map_err(|_| EgressError::Connect)?
                    .to_vec();
                self.record(&host, req.method, req.url.path(), header_names, true);
                Ok(EgressResponse {
                    status,
                    headers,
                    body,
                })
            }
            Some(Route::Scripted(q)) => {
                let mut q = q.clone();
                let r = q.pop_front().unwrap_or(Err(EgressError::Connect));
                self.record(&host, req.method, req.url.path(), header_names, true);
                r
            }
            None => {
                self.record(&host, req.method, req.url.path(), header_names, false);
                Err(EgressError::HostNotAllowed)
            }
        }
    }
}
