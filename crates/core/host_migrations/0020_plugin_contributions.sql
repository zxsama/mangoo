BEGIN IMMEDIATE;
CREATE TABLE plugin_configuration (
 plugin_id TEXT PRIMARY KEY,
 values_json TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(values_json)),
 revision INTEGER NOT NULL DEFAULT 1 CHECK(revision > 0),
 updated_at INTEGER NOT NULL
);
CREATE TABLE plugin_configuration_plans (
 plan_uuid TEXT PRIMARY KEY,
 plugin_id TEXT NOT NULL,
 expected_revision INTEGER NOT NULL,
 requested_json TEXT NOT NULL CHECK(json_valid(requested_json)),
 effective_json TEXT NOT NULL CHECK(json_valid(effective_json)),
 declaration_json TEXT NOT NULL CHECK(json_valid(declaration_json)),
 lifecycle_epoch INTEGER NOT NULL,
 effective_changed INTEGER NOT NULL CHECK(effective_changed IN (0,1)),
 affected_libraries INTEGER NOT NULL,
 requires_confirmation INTEGER NOT NULL CHECK(requires_confirmation IN (0,1)),
 expires_at INTEGER NOT NULL,
 created_at INTEGER NOT NULL
);
CREATE INDEX idx_plugin_configuration_plans_plugin ON plugin_configuration_plans(plugin_id,expires_at);
INSERT INTO plugin_configuration(plugin_id,values_json,revision,updated_at)
 SELECT 'com.mangoo.onedrive',CASE WHEN host_override_client_id IS NULL THEN '{}' ELSE json_object('clientId',host_override_client_id) END,revision,updated_at FROM onedrive_configuration WHERE singleton=1;
DELETE FROM onedrive_configuration_plans;
ALTER TABLE plugin_secret_handles ADD COLUMN role TEXT NOT NULL DEFAULT 'scoped' CHECK(role IN ('scoped','primary'));
INSERT OR IGNORE INTO plugin_secret_handles(plugin_id,account_uuid,secret_ref,namespace,purpose,created_at,role)
 SELECT plugin_id,account_uuid,secret_ref,'microsoft-onedrive','oauth-v1',updated_at,'primary' FROM plugin_accounts WHERE plugin_id='com.mangoo.onedrive' AND secret_ref IS NOT NULL;
INSERT OR IGNORE INTO plugin_secret_handles(plugin_id,account_uuid,secret_ref,namespace,purpose,created_at,role)
 SELECT a.plugin_id,a.account_uuid,f.prior_secret_ref,'microsoft-onedrive','oauth-v1',a.updated_at,'primary' FROM plugin_accounts a JOIN plugin_auth_flow_ownership f ON f.plugin_id=a.plugin_id WHERE a.plugin_id='com.mangoo.onedrive' AND f.prior_secret_ref IS NOT NULL;
UPDATE plugin_secret_handles SET role='primary' WHERE plugin_id='com.mangoo.onedrive' AND namespace='microsoft-onedrive' AND purpose='oauth-v1' AND (EXISTS(SELECT 1 FROM plugin_accounts a WHERE a.plugin_id=plugin_secret_handles.plugin_id AND a.account_uuid=plugin_secret_handles.account_uuid AND a.secret_ref=plugin_secret_handles.secret_ref) OR EXISTS(SELECT 1 FROM plugin_auth_flow_ownership f WHERE f.plugin_id=plugin_secret_handles.plugin_id AND f.prior_secret_ref=plugin_secret_handles.secret_ref));
UPDATE plugin_versions SET manifest_json=json_set(manifest_json,'$.contributions',json('{"configuration":[{"id":"clientId","label":"Microsoft Entra Client ID","description":"应用程序(客户端) ID","format":"uuid","required":true,"environment":"MANGOO_ONEDRIVE_CLIENT_ID","defaultValue":null,"choices":[],"hidden":false,"debugOnly":false},{"id":"authority","label":"授权服务","description":"","format":"https-url","required":false,"environment":"MANGOO_ONEDRIVE_AUTHORITY","defaultValue":"https://login.microsoftonline.com/common/","choices":[],"hidden":true,"debugOnly":false},{"id":"authMode","label":"授权方式","description":"","format":"choice","required":false,"environment":"MANGOO_ONEDRIVE_AUTH_MODE","defaultValue":"loopback","choices":["loopback","manual-response-uri"],"hidden":true,"debugOnly":false},{"id":"emulatorUrl","label":"测试服务","description":"","format":"loopback-url","required":false,"environment":"MANGOO_ONEDRIVE_EMULATOR_URL","defaultValue":null,"choices":[],"hidden":true,"debugOnly":true}],"authentication":{"modes":["loopback","manual-response-uri"],"authorizationHosts":["login.microsoftonline.com"],"manualRedirectUri":"https://login.microsoftonline.com/common/oauth2/nativeclient","loopbackRedirectPath":"/plugin-auth/callback","ignoredCallbackFields":["session_state"]},"presentation":{"setupLabel":"配置 OneDrive","accountLabel":"OneDrive","icon":"cloud","helpLinks":[{"id":"registration","label":"Microsoft Entra 应用注册","url":"https://entra.microsoft.com/"}],"setupSteps":[{"title":"注册应用","body":"在 Microsoft Entra 的 App registrations 中选择 New registration，支持组织目录和个人 Microsoft 账户。","helpLinkId":"registration"},{"title":"配置身份验证","body":"在 Authentication 中添加 Mobile and desktop applications 平台，Redirect URI 使用下方此设备的返回地址，并启用 Allow public client flows。","helpLinkId":null},{"title":"设置权限","body":"添加 Microsoft Graph 委托权限 Files.ReadWrite.AppFolder，然后复制 Application (client) ID。","helpLinkId":null}]},"primaryCredential":{"namespace":"microsoft-onedrive","purpose":"oauth-v1"}}')) WHERE plugin_id='com.mangoo.onedrive' AND json_type(manifest_json,'$.contributions') IS NULL;
UPDATE host_meta SET schema_version=20 WHERE singleton=1;
COMMIT;
