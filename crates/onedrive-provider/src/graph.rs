use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom, Write};
use std::time::{Duration, SystemTime};

use reqwest::blocking::{Client, Response};
use reqwest::header::{
    AUTHORIZATION, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, LOCATION, RANGE, RETRY_AFTER,
};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use url::Url;

use crate::config::ProviderConfig;
use crate::error::{map_http_error, ProviderError};
use crate::namespace::{graph_path, validate_relative_key};
use crate::{MAX_OBJECT_BYTES, UPLOAD_FRAGMENT_ALIGNMENT, UPLOAD_FRAGMENT_BYTES};

const MAX_URL_BYTES: usize = 8192;
const MAX_REDIRECTS: usize = 3;
const MAX_LIBRARY_DESCRIPTOR_BYTES: u64 = 1024 * 1024;
const MAX_JSON_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StableIdentity {
    pub drive_id: String,
    pub account_kind: String,
    pub tenant_hint: Option<String>,
    pub object_hint: Option<String>,
    pub display_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteObject {
    pub key: String,
    pub size: u64,
    pub sha256: String,
    pub etag: Option<String>,
}

#[derive(Debug)]
pub struct Download {
    pub metadata: RemoteObject,
    pub file: tempfile::NamedTempFile,
    pub transfer_size: u64,
    pub transfer_sha256: [u8; 32],
    pub response_start: u64,
    pub response_end: u64,
    pub whole_object: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadRecordV1 {
    pub format_version: u8,
    pub upload_url: String,
    pub expires_at: Option<String>,
    pub namespace: String,
    pub object_key: String,
    pub size: u64,
    pub sha256: String,
    pub committed_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct GraphClient {
    http: Client,
    endpoint: Url,
    allow_http_emulator: bool,
}

impl GraphClient {
    pub fn new(config: &ProviderConfig) -> Result<Self, ProviderError> {
        let http = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|_| ProviderError::new("internal", false, "failed to build Graph client"))?;
        Ok(Self {
            http,
            endpoint: config.graph_endpoint.clone(),
            allow_http_emulator: config.allow_http_emulator,
        })
    }

    pub fn exchange_code(
        &self,
        config: &ProviderConfig,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<Value, ProviderError> {
        self.token_request(
            config,
            &[
                ("client_id", config.client_id.as_str()),
                ("grant_type", "authorization_code"),
                ("code", code),
                ("code_verifier", verifier),
                ("redirect_uri", redirect_uri),
                ("scope", crate::oauth::OAUTH_SCOPES),
            ],
        )
    }

    pub fn refresh_token(
        &self,
        config: &ProviderConfig,
        refresh_token: &str,
    ) -> Result<Value, ProviderError> {
        self.token_request(
            config,
            &[
                ("client_id", config.client_id.as_str()),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("scope", crate::oauth::OAUTH_SCOPES),
            ],
        )
    }

    fn token_request(
        &self,
        config: &ProviderConfig,
        form: &[(&str, &str)],
    ) -> Result<Value, ProviderError> {
        let response = self
            .http
            .post(config.token_endpoint()?)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .form(form)
            .send()
            .map_err(network_error)?;
        decode_json_response(response)
    }

    pub fn drive_identity(&self, access_token: &str) -> Result<StableIdentity, ProviderError> {
        let url = self.api_url("me/drive?$select=id,driveType,owner")?;
        let value = self.graph_json(Method::GET, url, access_token, None, None)?;
        let drive_id = string(&value, "id")?;
        Ok(StableIdentity {
            drive_id,
            account_kind: value
                .get("driveType")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_owned(),
            tenant_hint: value
                .pointer("/owner/user/id")
                .and_then(Value::as_str)
                .map(anonymize),
            object_hint: value
                .pointer("/owner/user/id")
                .and_then(Value::as_str)
                .map(anonymize),
            display_hint: value
                .pointer("/owner/user/displayName")
                .and_then(Value::as_str)
                .map(|v| v.chars().take(128).collect()),
        })
    }

    pub fn ensure_app_root(&self, access_token: &str) -> Result<String, ProviderError> {
        let approot = self.api_url("me/drive/special/approot?$select=id")?;
        let value = self.graph_json(Method::GET, approot, access_token, None, None)?;
        let mut parent = string(&value, "id")?;
        for name in ["Megle", "libraries"] {
            let url = self.api_url(&format!("me/drive/items/{parent}/children"))?;
            let body = json!({
                "name": name,
                "folder": {},
                "@microsoft.graph.conflictBehavior": "fail"
            });
            match self.graph_json(Method::POST, url, access_token, Some(body), None) {
                Ok(value) => parent = string(&value, "id")?,
                Err(error) if error.code == "precondition_failed" => {
                    let children = self.api_url(&format!(
                        "me/drive/items/{parent}/children?$select=id,name,folder"
                    ))?;
                    let values =
                        self.graph_json(Method::GET, children, access_token, None, None)?;
                    parent = values
                        .get("value")
                        .and_then(Value::as_array)
                        .and_then(|items| {
                            items
                                .iter()
                                .find(|item| item.get("name").and_then(Value::as_str) == Some(name))
                        })
                        .and_then(|item| item.get("id").and_then(Value::as_str))
                        .map(str::to_owned)
                        .ok_or(error)?;
                }
                Err(error) => return Err(error),
            }
        }
        Ok(parent)
    }

    pub fn delta_list(
        &self,
        access_token: &str,
        namespace: Option<&str>,
        cursor: Option<&str>,
        page_token: Option<&str>,
        limit: usize,
    ) -> Result<Value, ProviderError> {
        let url = if let Some(link) = page_token.or(cursor) {
            self.validate_graph_link(link)?
        } else {
            let libraries_id = self.ensure_app_root(access_token)?;
            let folder_id = match namespace {
                Some(namespace) => {
                    self.ensure_library_folder(access_token, &libraries_id, namespace)?
                }
                None => libraries_id,
            };
            self.api_url(&format!(
                "me/drive/items/{folder_id}/delta?$top={}",
                limit.clamp(1, 200)
            ))?
        };
        let value = self.graph_json(Method::GET, url, access_token, None, None)?;
        let mut objects = Vec::new();
        let mut discovered_libraries = HashSet::new();
        for item in value
            .get("value")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if item.get("deleted").is_some() {
                continue;
            }
            let Some(name) = item.get("name").and_then(Value::as_str) else {
                continue;
            };
            if namespace.is_none() && item.get("folder").is_some() {
                if !is_direct_library_folder(item) {
                    continue;
                }
                let Ok(library_uuid) = uuid::Uuid::parse_str(name) else {
                    continue;
                };
                if library_uuid.to_string() != name {
                    continue;
                }
                if !discovered_libraries.insert(name.to_owned()) {
                    return Err(ProviderError::protocol(
                        "duplicate OneDrive Library namespace",
                    ));
                }
                let Some(mut descriptor) = self.valid_library_descriptor(access_token, name)?
                else {
                    continue;
                };
                descriptor.key = format!("{name}/library-descriptor");
                objects.push(object_value(descriptor));
                continue;
            }
            if item.get("folder").is_some() {
                continue;
            }
            let Some(relative_key) = namespace
                .map(|namespace| delta_relative_key(item, namespace))
                .transpose()?
                .flatten()
            else {
                continue;
            };
            let Some(size) = item.get("size").and_then(Value::as_u64) else {
                continue;
            };
            let hash = expected_hash_from_key(&relative_key)
                .map(Ok)
                .unwrap_or_else(|| {
                    let namespace =
                        namespace.expect("root Delta objects are handled as Library folders");
                    match self.head(access_token, namespace, &relative_key)? {
                        Some(metadata) if metadata.size == size => Ok(metadata.sha256),
                        Some(_) => Err(ProviderError::protocol(
                            "Graph delta size changed during validation",
                        )),
                        None => Err(ProviderError::protocol(
                            "Graph Delta object disappeared during validation",
                        )),
                    }
                })?;
            objects.push(json!({"key":relative_key,"size":size,"sha256":hash,"etag":item.get("eTag").and_then(Value::as_str)}));
        }
        let next = value
            .get("@odata.nextLink")
            .and_then(Value::as_str)
            .map(|v| self.validate_graph_link(v).map(|u| u.to_string()))
            .transpose()?;
        let completed = value
            .get("@odata.deltaLink")
            .and_then(Value::as_str)
            .map(|v| self.validate_graph_link(v).map(|u| u.to_string()))
            .transpose()?;
        Ok(json!({"objects":objects,"pageToken":next,"completedCursor":completed}))
    }

    fn valid_library_descriptor(
        &self,
        access_token: &str,
        namespace: &str,
    ) -> Result<Option<RemoteObject>, ProviderError> {
        let mut download = match self.download(
            access_token,
            namespace,
            "library-descriptor",
            None,
            MAX_LIBRARY_DESCRIPTOR_BYTES,
        ) {
            Ok(download) => download,
            Err(error) if error.code == "not_found" => return Ok(None),
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::with_capacity(download.transfer_size as usize);
        download
            .file
            .as_file_mut()
            .read_to_end(&mut bytes)
            .map_err(body_io_error)?;
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| ProviderError::protocol("invalid OneDrive Library descriptor"))?;
        let valid = value.get("version").and_then(Value::as_u64) == Some(1)
            && value.get("libraryUuid").and_then(Value::as_str) == Some(namespace)
            && value.get("namespace").and_then(Value::as_str) == Some(namespace)
            && value
                .pointer("/seed/objectKey")
                .and_then(Value::as_str)
                .is_some_and(|key| validate_relative_key(key).is_ok())
            && value
                .pointer("/seed/sha256")
                .and_then(Value::as_str)
                .is_some_and(valid_sha256)
            && value
                .get("dependencies")
                .and_then(Value::as_array)
                .is_some_and(|dependencies| {
                    dependencies.iter().all(|value| {
                        value
                            .as_str()
                            .is_some_and(|key| validate_relative_key(key).is_ok())
                    })
                });
        if !valid {
            return Err(ProviderError::protocol(
                "invalid OneDrive Library descriptor",
            ));
        }
        let seed_key = value
            .pointer("/seed/objectKey")
            .and_then(Value::as_str)
            .unwrap();
        let seed_hash = value
            .pointer("/seed/sha256")
            .and_then(Value::as_str)
            .unwrap();
        let seed = self.head(access_token, namespace, seed_key)?;
        if !seed.is_some_and(|object| object.sha256 == seed_hash) {
            return Ok(None);
        }
        for dependency in value["dependencies"].as_array().unwrap() {
            let key = dependency.as_str().unwrap();
            if self.head(access_token, namespace, key)?.is_none() {
                return Ok(None);
            }
        }
        Ok(Some(download.metadata))
    }

    pub fn head(
        &self,
        access_token: &str,
        namespace: &str,
        key: &str,
    ) -> Result<Option<RemoteObject>, ProviderError> {
        let url = self.item_url(namespace, key, "?$select=id,name,size,eTag")?;
        let response = self.send_graph(Method::GET, url, access_token, None, None)?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let value = decode_json_response(response)?;
        let size = value
            .get("size")
            .and_then(Value::as_u64)
            .ok_or_else(|| ProviderError::protocol("Graph object has no size"))?;
        if size > MAX_OBJECT_BYTES {
            return Err(ProviderError::protocol(
                "Graph object exceeds Provider limit",
            ));
        }
        let hash = self.hash_object_body(access_token, namespace, key, size)?;
        Ok(Some(RemoteObject {
            key: key.to_owned(),
            size,
            sha256: hash,
            etag: value.get("eTag").and_then(Value::as_str).map(str::to_owned),
        }))
    }

    fn hash_object_body(
        &self,
        access_token: &str,
        namespace: &str,
        key: &str,
        size: u64,
    ) -> Result<String, ProviderError> {
        let content_url = self.item_url(namespace, key, ":/content")?;
        let first = self.send_graph(Method::GET, content_url, access_token, None, None)?;
        let response = if first.status().is_redirection() {
            let location = first
                .headers()
                .get(LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| {
                    ProviderError::protocol("Graph download redirect is missing Location")
                })?;
            self.send_capability(
                Method::GET,
                validate_capability_url(location, self.allow_http_emulator)?,
                None,
                None,
            )?
        } else {
            first
        };
        if !response.status().is_success() {
            return Err(response_error(response));
        }
        let (body_size, body_hash) = hash_bounded(response, size)?;
        if body_size != size {
            return Err(ProviderError::protocol("Graph object body is truncated"));
        }
        Ok(hex(&body_hash))
    }

    pub fn download(
        &self,
        access_token: &str,
        namespace: &str,
        key: &str,
        range: Option<(u64, u64)>,
        max_bytes: u64,
    ) -> Result<Download, ProviderError> {
        let metadata = match expected_hash_from_key(key) {
            Some(sha256) => {
                let url = self.item_url(namespace, key, "?$select=id,name,size,eTag")?;
                let response = self.send_graph(Method::GET, url, access_token, None, None)?;
                if response.status() == StatusCode::NOT_FOUND {
                    return Err(ProviderError::new(
                        "not_found",
                        false,
                        "OneDrive object was not found",
                    ));
                }
                let value = decode_json_response(response)?;
                let size = value
                    .get("size")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| ProviderError::protocol("Graph object has no size"))?;
                RemoteObject {
                    key: key.to_owned(),
                    size,
                    sha256,
                    etag: value.get("eTag").and_then(Value::as_str).map(str::to_owned),
                }
            }
            None => self.head(access_token, namespace, key)?.ok_or_else(|| {
                ProviderError::new("not_found", false, "OneDrive object was not found")
            })?,
        };
        if metadata.size > MAX_OBJECT_BYTES || max_bytes > MAX_OBJECT_BYTES {
            return Err(ProviderError::new(
                "invalid_argument",
                false,
                "object exceeds Provider limit",
            ));
        }
        let content_url = self.item_url(namespace, key, ":/content")?;
        let first = self.send_graph(Method::GET, content_url, access_token, None, range)?;
        let response = if first.status().is_redirection() {
            let location = first
                .headers()
                .get(LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| {
                    ProviderError::protocol("Graph download redirect is missing Location")
                })?;
            let url = validate_capability_url(location, self.allow_http_emulator)?;
            self.send_capability(Method::GET, url, None, range)?
        } else {
            first
        };
        let status = response.status();
        if !status.is_success() {
            return Err(response_error(response));
        }
        let whole_object = range.is_some() && status == StatusCode::OK;
        let (response_start, response_end) = match range {
            None => (0, metadata.size),
            Some((start, end)) if status == StatusCode::PARTIAL_CONTENT => {
                let content_range = response
                    .headers()
                    .get(CONTENT_RANGE)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| {
                        ProviderError::protocol("ranged download has no Content-Range")
                    })?;
                let parsed = parse_content_range(content_range)?;
                if parsed != (start, end, metadata.size) {
                    return Err(ProviderError::protocol("ranged download interval mismatch"));
                }
                (start, end)
            }
            Some(_) if whole_object => (0, metadata.size),
            Some(_) => return Err(ProviderError::protocol("unexpected range response status")),
        };
        let cap = if whole_object {
            max_bytes
        } else {
            response_end - response_start
        };
        let (mut file, transfer_size, transfer_sha256) = spool_bounded(response, cap)?;
        if transfer_size != response_end - response_start {
            return Err(ProviderError::protocol("download body is truncated"));
        }
        if whole_object && hex(&transfer_sha256) != metadata.sha256 {
            return Err(ProviderError::protocol(
                "whole-object fallback hash mismatch",
            ));
        }
        file.as_file_mut()
            .seek(SeekFrom::Start(0))
            .map_err(body_io_error)?;
        Ok(Download {
            metadata,
            file,
            transfer_size,
            transfer_sha256,
            response_start,
            response_end,
            whole_object,
        })
    }

    pub fn put_immutable(
        &self,
        access_token: &str,
        namespace: &str,
        key: &str,
        bytes: &[u8],
        expected_hash: &str,
    ) -> Result<Value, ProviderError> {
        if bytes.len() as u64 > MAX_OBJECT_BYTES || hex(&Sha256::digest(bytes)) != expected_hash {
            return Err(ProviderError::new(
                "invalid_argument",
                false,
                "immutable upload identity mismatch",
            ));
        }
        let url = self.item_url(
            namespace,
            key,
            ":/content?@microsoft.graph.conflictBehavior=fail",
        )?;
        let response =
            self.send_graph(Method::PUT, url, access_token, Some(bytes.to_vec()), None)?;
        if response.status().is_success() {
            let value = decode_json_response(response)?;
            return Ok(
                json!({"status":"created","etag":value.get("eTag").and_then(Value::as_str)}),
            );
        }
        if matches!(
            response.status(),
            StatusCode::CONFLICT | StatusCode::PRECONDITION_FAILED
        ) {
            let existing = self
                .head(access_token, namespace, key)?
                .ok_or_else(|| ProviderError::protocol("conflicting object disappeared"))?;
            let actual = existing.sha256.clone();
            return Ok(
                if existing.size == bytes.len() as u64 && actual == expected_hash {
                    json!({"status":"already_exists_matching","etag":existing.etag})
                } else {
                    json!({"status":"hash_conflict","remoteSha256":actual})
                },
            );
        }
        Err(response_error(response))
    }

    pub fn begin_upload(
        &self,
        access_token: &str,
        namespace: &str,
        key: &str,
        size: u64,
        sha256: &str,
    ) -> Result<UploadRecordV1, ProviderError> {
        if size == 0 || size > MAX_OBJECT_BYTES || !valid_sha256(sha256) {
            return Err(ProviderError::new(
                "invalid_argument",
                false,
                "invalid immutable upload identity",
            ));
        }
        let url = self.item_url(namespace, key, ":/createUploadSession")?;
        let response = self.send_graph(
            Method::POST,
            url,
            access_token,
            Some(
                serde_json::to_vec(&json!({"item":{"@microsoft.graph.conflictBehavior":"fail"}}))
                    .unwrap(),
            ),
            None,
        )?;
        let value = decode_json_response(response)?;
        let upload_url = string(&value, "uploadUrl")?;
        validate_capability_url(&upload_url, self.allow_http_emulator)?;
        Ok(UploadRecordV1 {
            format_version: 1,
            upload_url,
            expires_at: value
                .get("expirationDateTime")
                .and_then(Value::as_str)
                .map(str::to_owned),
            namespace: namespace.to_owned(),
            object_key: key.to_owned(),
            size,
            sha256: sha256.to_owned(),
            committed_bytes: 0,
        })
    }

    pub fn upload_fragment(
        &self,
        record: &mut UploadRecordV1,
        offset: u64,
        bytes: &[u8],
    ) -> Result<u64, ProviderError> {
        if offset != record.committed_bytes
            || bytes.is_empty()
            || bytes.len() > UPLOAD_FRAGMENT_BYTES
        {
            return Err(ProviderError::new(
                "invalid_argument",
                false,
                "upload fragment offset or size is invalid",
            ));
        }
        let final_fragment = offset + bytes.len() as u64 == record.size;
        if !final_fragment && bytes.len() % UPLOAD_FRAGMENT_ALIGNMENT != 0 {
            return Err(ProviderError::new(
                "invalid_argument",
                false,
                "non-final upload fragment is not 320 KiB aligned",
            ));
        }
        let url = validate_capability_url(&record.upload_url, self.allow_http_emulator)?;
        let range = format!(
            "bytes {}-{}/{}",
            offset,
            offset + bytes.len() as u64 - 1,
            record.size
        );
        let response = self
            .http
            .put(url)
            .header(CONTENT_LENGTH, bytes.len())
            .header(CONTENT_RANGE, range)
            .body(bytes.to_vec())
            .send()
            .map_err(network_error)?;
        if response.status().is_success() {
            let final_response = response.status() != StatusCode::ACCEPTED;
            let value = decode_json_response(response)?;
            if let Some(next) = next_expected_offset(&value) {
                if next < offset || next > record.size {
                    return Err(ProviderError::protocol(
                        "upload response acknowledged an invalid offset",
                    ));
                }
                record.committed_bytes = next;
            } else if final_response && value.get("id").is_some() {
                record.committed_bytes = record.size;
            } else {
                return Err(ProviderError::protocol(
                    "upload response has no acknowledged range",
                ));
            }
            return Ok(record.committed_bytes);
        }
        if response.status() == StatusCode::RANGE_NOT_SATISFIABLE
            || response.status().is_server_error()
        {
            record.committed_bytes = self
                .upload_status(record)?
                .unwrap_or(record.committed_bytes);
            return Ok(record.committed_bytes);
        }
        Err(response_error(response))
    }

    pub fn upload_status(&self, record: &UploadRecordV1) -> Result<Option<u64>, ProviderError> {
        let url = validate_capability_url(&record.upload_url, self.allow_http_emulator)?;
        let response = self.http.get(url).send().map_err(network_error)?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let offset = next_expected_offset(&decode_json_response(response)?);
        if offset.is_some_and(|offset| offset > record.size) {
            return Err(ProviderError::protocol(
                "upload status acknowledged an invalid offset",
            ));
        }
        Ok(offset)
    }

    pub fn abort_upload(&self, record: &UploadRecordV1) -> Result<(), ProviderError> {
        let url = validate_capability_url(&record.upload_url, self.allow_http_emulator)?;
        let response = self.http.delete(url).send().map_err(network_error)?;
        if response.status().is_success() || response.status() == StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(response_error(response))
        }
    }

    fn ensure_library_folder(
        &self,
        access_token: &str,
        libraries_id: &str,
        namespace: &str,
    ) -> Result<String, ProviderError> {
        uuid::Uuid::parse_str(namespace).map_err(|_| {
            ProviderError::new("invalid_argument", false, "invalid Library namespace")
        })?;
        let url = self.api_url(&format!("me/drive/items/{libraries_id}/children"))?;
        let body = json!({"name":namespace,"folder":{},"@microsoft.graph.conflictBehavior":"fail"});
        match self.graph_json(Method::POST, url, access_token, Some(body), None) {
            Ok(value) => string(&value, "id"),
            Err(error) if error.code == "precondition_failed" => {
                let children = self.api_url(&format!(
                    "me/drive/items/{libraries_id}/children?$select=id,name,folder"
                ))?;
                let value = self.graph_json(Method::GET, children, access_token, None, None)?;
                value
                    .get("value")
                    .and_then(Value::as_array)
                    .and_then(|items| {
                        items.iter().find(|item| {
                            item.get("name").and_then(Value::as_str) == Some(namespace)
                        })
                    })
                    .and_then(|item| item.get("id").and_then(Value::as_str))
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        ProviderError::protocol("Library folder conflict could not be reconciled")
                    })
            }
            Err(error) => Err(error),
        }
    }

    fn item_url(&self, namespace: &str, key: &str, suffix: &str) -> Result<Url, ProviderError> {
        let path = graph_path(namespace, key)?;
        self.api_url(&format!("me/drive/special/approot:/{path}{suffix}"))
    }

    fn api_url(&self, path: &str) -> Result<Url, ProviderError> {
        self.endpoint
            .join(path)
            .map_err(|_| ProviderError::protocol("invalid Graph API path"))
    }

    fn validate_graph_link(&self, value: &str) -> Result<Url, ProviderError> {
        if value.len() > MAX_URL_BYTES {
            return Err(ProviderError::protocol("Graph paging URL is too long"));
        }
        let url =
            Url::parse(value).map_err(|_| ProviderError::protocol("invalid Graph paging URL"))?;
        if url.username() != "" || url.password().is_some() || url.fragment().is_some() {
            return Err(ProviderError::protocol("invalid Graph paging URL"));
        }
        if self.allow_http_emulator && url.origin() == self.endpoint.origin() {
            return Ok(url);
        }
        if url.scheme() != "https"
            || url.host_str() != Some("graph.microsoft.com")
            || url.port_or_known_default() != Some(443)
            || !url.path().starts_with("/v1.0/")
        {
            return Err(ProviderError::protocol(
                "Graph paging URL escaped the allowed origin",
            ));
        }
        Ok(url)
    }

    fn graph_json(
        &self,
        method: Method,
        url: Url,
        token: &str,
        body: Option<Value>,
        range: Option<(u64, u64)>,
    ) -> Result<Value, ProviderError> {
        let body = body.map(|v| serde_json::to_vec(&v).unwrap());
        decode_json_response(self.send_graph(method, url, token, body, range)?)
    }

    fn send_graph(
        &self,
        method: Method,
        url: Url,
        token: &str,
        body: Option<Vec<u8>>,
        range: Option<(u64, u64)>,
    ) -> Result<Response, ProviderError> {
        self.validate_graph_link(url.as_str())?;
        let mut request = self
            .http
            .request(method, url)
            .header(AUTHORIZATION, format!("Bearer {token}"));
        if let Some(bytes) = body {
            request = request.header(CONTENT_TYPE, "application/json").body(bytes);
        }
        if let Some((start, end)) = range {
            if end <= start {
                return Err(ProviderError::new(
                    "invalid_argument",
                    false,
                    "invalid download range",
                ));
            }
            request = request.header(RANGE, format!("bytes={start}-{}", end - 1));
        }
        request.send().map_err(network_error)
    }

    fn send_capability(
        &self,
        method: Method,
        mut url: Url,
        body: Option<Vec<u8>>,
        range: Option<(u64, u64)>,
    ) -> Result<Response, ProviderError> {
        for redirect in 0..=MAX_REDIRECTS {
            let mut request = self.http.request(method.clone(), url.clone());
            if let Some(bytes) = body.clone() {
                request = request.body(bytes);
            }
            if let Some((start, end)) = range {
                request = request.header(RANGE, format!("bytes={start}-{}", end - 1));
            }
            let response = request.send().map_err(network_error)?;
            if !response.status().is_redirection() {
                return Ok(response);
            }
            if redirect == MAX_REDIRECTS {
                return Err(ProviderError::protocol("too many capability redirects"));
            }
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| {
                    ProviderError::protocol("capability redirect is missing Location")
                })?;
            url = validate_capability_url(location, self.allow_http_emulator)?;
        }
        unreachable!()
    }
}

fn decode_json_response(response: Response) -> Result<Value, ProviderError> {
    if !response.status().is_success() {
        return Err(response_error(response));
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_JSON_RESPONSE_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(body_io_error)?;
    if bytes.len() as u64 > MAX_JSON_RESPONSE_BYTES {
        return Err(ProviderError::protocol("Graph JSON response is too large"));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| ProviderError::protocol("Graph returned malformed JSON"))
}

fn response_error(response: Response) -> ProviderError {
    let status = response.status().as_u16();
    let retry_after = parse_retry_after(
        response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|v| v.to_str().ok()),
    );
    let mut bytes = Vec::new();
    let value = if response
        .take(MAX_JSON_RESPONSE_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .is_ok()
        && bytes.len() as u64 <= MAX_JSON_RESPONSE_BYTES
    {
        serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    let code = value
        .pointer("/error/code")
        .and_then(Value::as_str)
        .or_else(|| value.get("error").and_then(Value::as_str));
    map_http_error(status, code, retry_after)
}

fn network_error(_error: reqwest::Error) -> ProviderError {
    ProviderError::new(
        "unavailable",
        true,
        "Microsoft Graph network request failed",
    )
}

fn hash_bounded(response: Response, max_bytes: u64) -> Result<(u64, [u8; 32]), ProviderError> {
    let mut reader = response.take(max_bytes.saturating_add(1));
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut size = 0u64;
    loop {
        let read = reader.read(&mut buffer).map_err(body_io_error)?;
        if read == 0 {
            break;
        }
        size = size.saturating_add(read as u64);
        if size > max_bytes {
            return Err(ProviderError::protocol(
                "download body exceeds caller limit",
            ));
        }
        digest.update(&buffer[..read]);
    }
    Ok((size, digest.finalize().into()))
}

fn spool_bounded(
    response: Response,
    max_bytes: u64,
) -> Result<(tempfile::NamedTempFile, u64, [u8; 32]), ProviderError> {
    // Core launches the Provider with its private runtime directory as cwd and
    // an intentionally cleared environment, so never depend on TEMP/TMP.
    let mut file = tempfile::NamedTempFile::new_in(".").map_err(body_io_error)?;
    let mut reader = response.take(max_bytes.saturating_add(1));
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut size = 0u64;
    loop {
        let read = reader.read(&mut buffer).map_err(body_io_error)?;
        if read == 0 {
            break;
        }
        size = size.saturating_add(read as u64);
        if size > max_bytes {
            return Err(ProviderError::protocol(
                "download body exceeds caller limit",
            ));
        }
        file.write_all(&buffer[..read]).map_err(body_io_error)?;
        digest.update(&buffer[..read]);
    }
    Ok((file, size, digest.finalize().into()))
}

fn body_io_error(_error: std::io::Error) -> ProviderError {
    ProviderError::new("unavailable", true, "Microsoft Graph response body failed")
}

pub fn validate_capability_url(
    value: &str,
    allow_http_emulator: bool,
) -> Result<Url, ProviderError> {
    if value.len() > MAX_URL_BYTES {
        return Err(ProviderError::protocol("capability URL is too long"));
    }
    let url = Url::parse(value).map_err(|_| ProviderError::protocol("invalid capability URL"))?;
    if url.username() != "" || url.password().is_some() || url.fragment().is_some() {
        return Err(ProviderError::protocol("invalid capability URL"));
    }
    if url.scheme() != "https" && !(allow_http_emulator && url.scheme() == "http") {
        return Err(ProviderError::protocol("capability URL must use HTTPS"));
    }
    let host = url
        .host_str()
        .ok_or_else(|| ProviderError::protocol("capability URL has no host"))?;
    if url.scheme() == "http" && !matches!(host, "127.0.0.1" | "localhost" | "::1") {
        return Err(ProviderError::protocol(
            "capability emulator URL must be loopback",
        ));
    }
    if url.scheme() == "https" && !trusted_capability_host(host) {
        return Err(ProviderError::protocol(
            "capability URL escaped Microsoft storage hosts",
        ));
    }
    if url.port().is_some_and(|port| {
        (url.scheme() == "https" && port != 443) || (url.scheme() == "http" && !allow_http_emulator)
    }) {
        return Err(ProviderError::protocol("capability URL has no host"));
    }
    Ok(url)
}

fn trusted_capability_host(host: &str) -> bool {
    [
        "1drv.com",
        "sharepoint.com",
        "sharepointonline.com",
        "microsoft.com",
    ]
    .iter()
    .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
}

fn is_direct_library_folder(item: &Value) -> bool {
    item.pointer("/parentReference/path")
        .and_then(Value::as_str)
        .is_some_and(|path| path.ends_with("/Megle/libraries"))
}

fn delta_relative_key(item: &Value, namespace: &str) -> Result<Option<String>, ProviderError> {
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ProviderError::protocol("Graph Delta item has no name"))?;
    if validate_relative_key(name).is_err() || name.contains('/') {
        return Ok(None);
    }
    let Some(parent_path) = item
        .pointer("/parentReference/path")
        .and_then(Value::as_str)
    else {
        return Ok(None);
    };
    let marker = format!("/Megle/libraries/{namespace}");
    let Some(index) = parent_path.rfind(&marker) else {
        return Ok(None);
    };
    let suffix = &parent_path[index + marker.len()..];
    if !suffix.is_empty() && !suffix.starts_with('/') {
        return Ok(None);
    }
    let key = if suffix.is_empty() {
        name.to_owned()
    } else {
        format!("{}/{name}", suffix.trim_start_matches('/'))
    };
    validate_relative_key(&key)?;
    Ok(Some(key))
}

pub fn parse_retry_after(value: Option<&str>) -> Option<u64> {
    let value = value?;
    if let Ok(seconds) = value.trim().parse::<u64>() {
        return Some(seconds.saturating_mul(1000));
    }
    let when = httpdate::parse_http_date(value).ok()?;
    Some(
        when.duration_since(SystemTime::now())
            .unwrap_or_default()
            .as_millis()
            .min(u64::MAX as u128) as u64,
    )
}

pub fn parse_content_range(value: &str) -> Result<(u64, u64, u64), ProviderError> {
    let value = value
        .strip_prefix("bytes ")
        .ok_or_else(|| ProviderError::protocol("invalid Content-Range"))?;
    let (range, total) = value
        .split_once('/')
        .ok_or_else(|| ProviderError::protocol("invalid Content-Range"))?;
    let (start, inclusive_end) = range
        .split_once('-')
        .ok_or_else(|| ProviderError::protocol("invalid Content-Range"))?;
    let start = start
        .parse::<u64>()
        .map_err(|_| ProviderError::protocol("invalid Content-Range"))?;
    let inclusive_end = inclusive_end
        .parse::<u64>()
        .map_err(|_| ProviderError::protocol("invalid Content-Range"))?;
    let total = total
        .parse::<u64>()
        .map_err(|_| ProviderError::protocol("invalid Content-Range"))?;
    Ok((
        start,
        inclusive_end
            .checked_add(1)
            .ok_or_else(|| ProviderError::protocol("invalid Content-Range"))?,
        total,
    ))
}

fn next_expected_offset(value: &Value) -> Option<u64> {
    value
        .get("nextExpectedRanges")?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .filter_map(|range| {
            range
                .split_once('-')
                .map(|v| v.0)
                .unwrap_or(range)
                .parse::<u64>()
                .ok()
        })
        .min()
}

fn expected_hash_from_key(key: &str) -> Option<String> {
    key.rsplit('/')
        .next()?
        .split(['.', '-'])
        .find(|part| valid_sha256(part))
        .map(str::to_ascii_lowercase)
}

fn string(value: &Value, key: &str) -> Result<String, ProviderError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ProviderError::protocol(format!("Graph response is missing {key}")))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn object_value(object: RemoteObject) -> Value {
    json!({
        "key": object.key,
        "size": object.size,
        "sha256": object.sha256,
        "etag": object.etag,
    })
}

fn anonymize(value: &str) -> String {
    hex(&Sha256::digest(value.as_bytes()))[..16].to_owned()
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|v| format!("{v:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freezes_fragment_and_range_contracts() {
        assert_eq!(UPLOAD_FRAGMENT_BYTES, 10 * 1024 * 1024);
        assert_eq!(UPLOAD_FRAGMENT_BYTES % UPLOAD_FRAGMENT_ALIGNMENT, 0);
        assert_eq!(
            parse_content_range("bytes 10-19/100").unwrap(),
            (10, 20, 100)
        );
        assert!(parse_content_range("items 10-19/100").is_err());
        let hash = "ab".repeat(32);
        assert_eq!(
            expected_hash_from_key(&format!("content/data/ab/{hash}.pack")).as_deref(),
            Some(hash.as_str())
        );
        assert_eq!(
            expected_hash_from_key(&format!("indexes/chunks/id/0-{hash}.idx")).as_deref(),
            Some(hash.as_str())
        );
        assert_eq!(expected_hash_from_key("checkpoints/id.checkpoint"), None);
    }

    #[test]
    fn retry_after_supports_seconds_and_http_date() {
        assert_eq!(parse_retry_after(Some("7")), Some(7_000));
        assert!(parse_retry_after(Some("Wed, 21 Oct 2037 07:28:00 GMT")).is_some());
        assert_eq!(parse_retry_after(Some("not-a-date")), None);
    }

    #[test]
    fn capability_urls_never_accept_credentials_or_plain_http() {
        assert!(validate_capability_url("http://example.invalid/upload", false).is_err());
        assert!(
            validate_capability_url("https://user:pass@example.invalid/upload", false).is_err()
        );
        assert!(validate_capability_url(
            "https://public.dm.files.1drv.com/upload?token=opaque",
            false
        )
        .is_ok());
        assert!(
            validate_capability_url("https://example.invalid/upload?token=opaque", false).is_err()
        );
        assert!(validate_capability_url("http://example.invalid/upload", true).is_err());
        assert!(validate_capability_url(
            "https://public.dm.files.1drv.com:444/upload?token=opaque",
            false
        )
        .is_err());
    }

    #[test]
    fn delta_paths_are_rebuilt_from_validated_parent_paths() {
        let namespace = "018f0000-0000-7000-8000-000000000001";
        let nested = json!({
            "name":"object.json",
            "parentReference":{"path":format!("/drive/root:/Apps/Megle/Megle/libraries/{namespace}/segments")}
        });
        assert_eq!(
            delta_relative_key(&nested, namespace).unwrap().as_deref(),
            Some("segments/object.json")
        );
        let escaped = json!({
            "name":"object.json",
            "parentReference":{"path":"/drive/root:/Apps/Megle/Megle/libraries/other/segments"}
        });
        assert_eq!(delta_relative_key(&escaped, namespace).unwrap(), None);
    }

    #[test]
    fn next_expected_ranges_use_lowest_acknowledged_gap() {
        let value = json!({"nextExpectedRanges":["20971520-","10485760-20971519"]});
        assert_eq!(next_expected_offset(&value), Some(10 * 1024 * 1024));
    }
}
