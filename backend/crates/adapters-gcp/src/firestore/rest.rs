//! Low-level Firestore REST calls: `get`, `commit`, `runQuery`.
//!
//! All JSON for queries is built with `serde_json::json!`, never by string
//! formatting. Document names, field values and cursors are never logged.

use std::sync::Arc;

use reqwest::Method;
use serde_json::{json, Map, Value};
use url::Url;

use crate::gcp_http::{GcpError, GcpHttp};

/// A Firestore document as returned by the REST API.
#[derive(Clone, Debug)]
pub struct Document {
    pub name: String,
    pub fields: Map<String, Value>,
    pub update_time: String,
}

/// A single write in a commit: an update, a delete, or a transform.
#[derive(Clone, Debug, Default)]
pub struct Write {
    pub update: Option<(String, Map<String, Value>)>,
    pub delete: Option<String>,
    pub update_mask: Option<Vec<String>>,
    pub update_transforms: Option<Vec<Value>>,
    pub current_document: Option<Value>,
}

/// The Firestore REST client bound to one project and database.
pub struct Firestore {
    http: Arc<GcpHttp>,
    /// The `.../documents` base URL.
    base: Url,
}

impl Firestore {
    /// Builds the base URL from the config and the client's emulator setting.
    pub fn new(http: Arc<GcpHttp>, project: &str, database: &str) -> Self {
        let base = match http.emulator() {
                Some(addr) => format!(
                    "http://{addr}/v1/projects/{project}/databases/{database}/documents"
                ),
                None => format!(
                    "https://firestore.googleapis.com/v1/projects/{project}/databases/{database}/documents"
                ),
            };
        Self {
            http,
            base: Url::parse(&base).unwrap_or_else(|_| panic!("valid base url")),
        }
    }

    /// The full document name for a collection and ID (the Firestore resource
    /// path, without scheme, host or the `/v1` API prefix).
    pub fn doc_name(&self, collection: &str, id: &str) -> String {
        let path = self
            .base
            .path()
            .trim_start_matches("/v1")
            .trim_start_matches('/');
        format!("{path}/{collection}/{id}")
    }

    /// `GET {name}`; a 404 gives `Ok(None)`.
    ///
    /// # Errors
    ///
    /// Returns a `GcpError` on transport or non-404 failure.
    pub async fn get(&self, name: &str) -> Result<Option<Document>, GcpError> {
        let mut url = self.base.clone();
        url.set_path(&format!("/v1/{name}"));
        match self
            .http
            .json::<Value, Value>(Method::GET, &url, None::<&Value>)
            .await
        {
            Ok(body) => Ok(Some(parse_document(&body)?)),
            Err(GcpError::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// `POST {base}:commit` with the given writes.
    ///
    /// # Errors
    ///
    /// Returns a `GcpError` mapped from the Firestore error envelope.
    pub async fn commit(&self, writes: &[Write]) -> Result<Value, GcpError> {
        let mut url = self.base.clone();
        url.set_path(&format!("{}:commit", self.base.path()));
        let body = json!({ "writes": writes.iter().map(write_to_json).collect::<Vec<_>>() });
        self.http
            .json::<Value, Value>(Method::POST, &url, Some(&body))
            .await
    }

    /// `POST {base}:runQuery` with a structured query; returns the documents.
    ///
    /// # Errors
    ///
    /// Returns a `GcpError` on failure.
    pub async fn run_query(
        &self,
        collection: &str,
        query: Value,
    ) -> Result<Vec<Document>, GcpError> {
        let mut url = self.base.clone();
        url.set_path(&format!("{}:runQuery", self.base.path()));
        let body = json!({ "structuredQuery": query });
        let resp: Value = self
            .http
            .json::<Value, Value>(Method::POST, &url, Some(&body))
            .await?;
        let arr = resp.as_array().ok_or(GcpError::BadResponse)?;
        let mut docs = Vec::new();
        for item in arr {
            if let Some(doc) = item.get("document") {
                docs.push(parse_document(doc)?);
            }
        }
        let _ = collection;
        Ok(docs)
    }
}

fn parse_document(body: &Value) -> Result<Document, GcpError> {
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or(GcpError::BadResponse)?
        .to_owned();
    let fields = body
        .get("fields")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    let update_time = body
        .get("updateTime")
        .and_then(|v| v.as_str())
        .ok_or(GcpError::BadResponse)?
        .to_owned();
    Ok(Document {
        name,
        fields,
        update_time,
    })
}

fn write_to_json(w: &Write) -> Value {
    let mut out = Map::new();
    if let Some((name, fields)) = &w.update {
        out.insert("update".into(), json!({ "name": name, "fields": fields }));
    }
    if let Some(name) = &w.delete {
        out.insert("delete".into(), json!(name));
    }
    if let Some(paths) = &w.update_mask {
        out.insert("updateMask".into(), json!({ "fieldPaths": paths }));
    }
    if let Some(transforms) = &w.update_transforms {
        out.insert("updateTransforms".into(), json!(transforms));
    }
    if let Some(cd) = &w.current_document {
        out.insert("currentDocument".into(), cd.clone());
    }
    Value::Object(out)
}
