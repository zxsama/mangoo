#[test]
#[ignore = "requires MEGLE_ONEDRIVE_LIVE_ACCESS_TOKEN for a credential-gated release evidence run"]
fn personal_or_business_drive_identity_is_reachable() {
    use megle_onedrive_provider::config::ProviderConfig;
    use megle_onedrive_provider::graph::GraphClient;
    use sha2::Digest;

    let token = std::env::var("MEGLE_ONEDRIVE_LIVE_ACCESS_TOKEN")
        .expect("MEGLE_ONEDRIVE_LIVE_ACCESS_TOKEN must be supplied by the operator");
    let config = ProviderConfig::from_env().expect("OneDrive client configuration");
    let identity = GraphClient::new(&config)
        .unwrap()
        .drive_identity(&token)
        .unwrap();
    let expected = std::env::var("MEGLE_ONEDRIVE_EXPECT_ACCOUNT_KIND").ok();
    assert!(matches!(
        identity.account_kind.as_str(),
        "personal" | "business"
    ));
    if let Some(expected) = expected {
        assert_eq!(identity.account_kind, expected);
    }
    eprintln!(
        "live OneDrive account kind: {}; drive hint: {}",
        identity.account_kind,
        &megle_onedrive_provider::graph::hex(&sha2::Sha256::digest(identity.drive_id.as_bytes()))
            [..16]
    );
}
