pub mod config;
pub mod error;
pub mod graph;
pub mod namespace;
pub mod oauth;
pub mod rpc;

pub const PLUGIN_ID: &str = "com.megle.onedrive";
pub const PLUGIN_VERSION: &str = "1.0.0";
pub const MAX_OBJECT_BYTES: u64 = 96 * 1024 * 1024;
pub const UPLOAD_FRAGMENT_BYTES: usize = 10 * 1024 * 1024;
pub const UPLOAD_FRAGMENT_ALIGNMENT: usize = 320 * 1024;
