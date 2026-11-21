//! Data format shared by Cargo's build script and the offline runtime. The
//! delimiters let the packager attest the actual compiled executable.
use std::collections::HashSet;

use serde::Deserialize;

pub const PREFIX: &[u8] = b"MANGOO-BUNDLED-TRUST-V1\0";
pub const SUFFIX: &[u8] = b"\0END-MANGOO-BUNDLED-TRUST";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Inventory {
    schema_version: u32,
    plugins: Vec<Row>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Row {
    id: String,
    version: String,
    platform: String,
    engine: Option<String>,
    sha256: Option<String>,
    manifest_sha256: String,
}

pub struct Entry {
    pub id: String,
    pub version: String,
    pub windows: bool,
    pub executable: [u8; 32],
    pub manifest: [u8; 32],
}

fn digest(value: &str) -> Result<[u8; 32], String> {
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid SHA-256".into());
    }
    let mut result = [0; 32];
    for (i, byte) in result.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[i * 2..i * 2 + 2], 16).map_err(|e| e.to_string())?;
    }
    if result == [0; 32] {
        return Err("zero artifact digest".into());
    }
    Ok(result)
}

pub fn entries(bytes: &[u8]) -> Result<Vec<Entry>, String> {
    let inventory: Inventory = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if inventory.schema_version != 1 {
        return Err("unsupported inventory version".into());
    }
    let mut ids = HashSet::new();
    inventory
        .plugins
        .into_iter()
        .map(|row| {
            if row.id.is_empty() || row.version.is_empty() || !ids.insert(row.id.clone()) {
                return Err("missing or duplicate plugin identity".into());
            }
            let executable = match (row.engine.as_deref(), row.sha256.as_deref()) {
                (Some("host"), None) => [0; 32],
                (None, Some(hash)) => digest(hash)?,
                _ => return Err("invalid host/process executable identity".into()),
            };
            let windows = match row.platform.as_str() {
                "windowsX64" => true,
                "linuxX64" => false,
                _ => return Err("unsupported platform".into()),
            };
            Ok(Entry {
                id: row.id,
                version: row.version,
                windows,
                executable,
                manifest: digest(&row.manifest_sha256)?,
            })
        })
        .collect()
}
