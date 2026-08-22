use std::sync::Arc;

use base64::Engine;
use serde_json::{json, Value};

use super::rpc::ReverseRpcHandler;
use crate::plugins::secret_store::{SecretScope, SecretStore};

pub struct SecretBroker {
    store: Arc<dyn SecretStore>,
    registry: Arc<crate::host_registry::HostRegistry>,
    plugin_id: String,
    account_uuid: String,
    launch_fingerprint: String,
}

impl SecretBroker {
    pub fn new(
        store: Arc<dyn SecretStore>,
        registry: Arc<crate::host_registry::HostRegistry>,
        plugin_id: String,
        account_uuid: String,
        launch_fingerprint: String,
    ) -> Self {
        Self {
            store,
            registry,
            plugin_id,
            account_uuid,
            launch_fingerprint,
        }
    }
    fn scope(&self, payload: &Value, operation: &str) -> Result<SecretScope, String> {
        if !self
            .registry
            .plugin_runtime_launch_is_current(&self.plugin_id, &self.launch_fingerprint)
            .map_err(|e| e.to_string())?
        {
            return Err("plugin runtime session is stale".into());
        }
        let namespace = payload
            .get("namespace")
            .and_then(Value::as_str)
            .ok_or("missing namespace")?;
        let purpose = payload
            .get("purpose")
            .and_then(Value::as_str)
            .ok_or("missing purpose")?;
        let projection = self
            .registry
            .host_plugin(&self.plugin_id)
            .map_err(|e| e.to_string())?
            .ok_or("plugin is not registered")?;
        let allowed = projection
            .granted_permissions
            .as_array()
            .is_some_and(|items| {
                items.iter().any(|item| {
                    item.get("kind").and_then(Value::as_str) == Some("secret")
                        && item.get("namespace").and_then(Value::as_str) == Some(namespace)
                        && item
                            .get("operations")
                            .and_then(Value::as_array)
                            .is_some_and(|operations| {
                                operations
                                    .iter()
                                    .any(|value| value.as_str() == Some(operation))
                            })
                })
            });
        if !allowed {
            return Err("secret namespace is not granted".into());
        }
        Ok(SecretScope {
            plugin_id: self.plugin_id.clone(),
            account_uuid: self.account_uuid.clone(),
            namespace: namespace.into(),
            purpose: purpose.into(),
        })
    }
}

impl ReverseRpcHandler for SecretBroker {
    fn handle(&self, method: &str, payload: &Value) -> Result<Value, String> {
        let operation = match method {
            "host.secret.current" | "host.secret.get" => "read",
            "host.secret.create"
            | "host.secret.cas"
            | "host.secret.create_scoped"
            | "host.secret.cas_scoped" => "write",
            "host.secret.delete" | "host.secret.delete_scoped" => "delete",
            _ => return Err("reverse RPC method is not allowed".into()),
        };
        let scope = self.scope(payload, operation)?;
        let decode = |key: &str| -> Result<Vec<u8>, String> {
            base64::engine::general_purpose::STANDARD
                .decode(
                    payload
                        .get(key)
                        .and_then(Value::as_str)
                        .ok_or("missing value")?,
                )
                .map_err(|_| "invalid base64 value".into())
        };
        match method {
            "host.secret.current" => {
                let account = self
                    .registry
                    .plugin_account_secret(&self.plugin_id)
                    .map_err(|e| e.to_string())?
                    .ok_or("plugin account is not registered")?;
                if account.account_uuid != self.account_uuid {
                    return Err("plugin account does not match runtime".into());
                }
                Ok(json!({"secretRef":account.secret_ref,"accountGeneration":account.generation}))
            }
            "host.secret.create" => {
                let reference = self
                    .store
                    .create(&scope, &decode("value")?)
                    .map_err(|e| e.to_string())?;
                if !self
                    .registry
                    .set_plugin_account_secret_for_launch(
                        &self.plugin_id,
                        &self.launch_fingerprint,
                        Some(&reference),
                        "authenticated",
                    )
                    .map_err(|e| e.to_string())?
                {
                    let _ = self.store.delete(&scope, &reference);
                    return Err("plugin runtime session is stale".into());
                }
                Ok(json!({"secretRef":reference,"generation":1}))
            }
            "host.secret.create_scoped" => {
                let reference = self
                    .store
                    .create(&scope, &decode("value")?)
                    .map_err(|e| e.to_string())?;
                Ok(json!({"secretRef":reference,"generation":1}))
            }
            "host.secret.get" => {
                let value = self
                    .store
                    .read(
                        &scope,
                        payload
                            .get("secretRef")
                            .and_then(Value::as_str)
                            .ok_or("missing secretRef")?,
                    )
                    .map_err(|e| e.to_string())?;
                Ok(
                    json!({"value":base64::engine::general_purpose::STANDARD.encode(value.bytes),"generation":value.generation}),
                )
            }
            "host.secret.cas" => {
                let next = self
                    .store
                    .compare_and_swap(
                        &scope,
                        payload
                            .get("secretRef")
                            .and_then(Value::as_str)
                            .ok_or("missing secretRef")?,
                        payload
                            .get("expectedGeneration")
                            .and_then(Value::as_u64)
                            .ok_or("missing generation")?,
                        &decode("value")?,
                    )
                    .map_err(|e| e.to_string())?;
                if !self
                    .registry
                    .set_plugin_account_secret_for_launch(
                        &self.plugin_id,
                        &self.launch_fingerprint,
                        payload.get("secretRef").and_then(Value::as_str),
                        "authenticated",
                    )
                    .map_err(|e| e.to_string())?
                {
                    return Err("plugin runtime session is stale".into());
                }
                Ok(json!({"generation":next}))
            }
            "host.secret.cas_scoped" => {
                let next = self
                    .store
                    .compare_and_swap(
                        &scope,
                        payload
                            .get("secretRef")
                            .and_then(Value::as_str)
                            .ok_or("missing secretRef")?,
                        payload
                            .get("expectedGeneration")
                            .and_then(Value::as_u64)
                            .ok_or("missing generation")?,
                        &decode("value")?,
                    )
                    .map_err(|e| e.to_string())?;
                Ok(json!({"generation":next}))
            }
            "host.secret.delete" => {
                let generation = self
                    .registry
                    .plugin_account_secret(&self.plugin_id)
                    .map_err(|e| e.to_string())?
                    .ok_or("plugin account is not registered")?
                    .generation
                    .checked_add(1)
                    .ok_or("secret generation exhausted")?;
                self.store
                    .delete(
                        &scope,
                        payload
                            .get("secretRef")
                            .and_then(Value::as_str)
                            .ok_or("missing secretRef")?,
                    )
                    .map_err(|e| e.to_string())?;
                if !self
                    .registry
                    .sign_out_plugin_account(&self.plugin_id, generation - 1)
                    .map_err(|e| e.to_string())?
                {
                    return Err("plugin runtime session is stale".into());
                }
                Ok(Value::Null)
            }
            "host.secret.delete_scoped" => {
                self.store
                    .delete(
                        &scope,
                        payload
                            .get("secretRef")
                            .and_then(Value::as_str)
                            .ok_or("missing secretRef")?,
                    )
                    .map_err(|e| e.to_string())?;
                Ok(Value::Null)
            }
            _ => Err("reverse RPC method is not allowed".into()),
        }
    }
}
