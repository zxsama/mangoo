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

#[test]
#[ignore = "requires MEGLE_ONEDRIVE_LIVE_ACCESS_TOKEN and writes isolated canary objects"]
fn delta_immutable_range_and_upload_session_round_trip() {
    use std::io::Read;

    use megle_onedrive_provider::config::ProviderConfig;
    use megle_onedrive_provider::graph::{hex, GraphClient};
    use sha2::{Digest, Sha256};

    let token = std::env::var("MEGLE_ONEDRIVE_LIVE_ACCESS_TOKEN")
        .expect("MEGLE_ONEDRIVE_LIVE_ACCESS_TOKEN must be supplied by the operator");
    let config = ProviderConfig::from_env().expect("OneDrive client configuration");
    let client = GraphClient::new(&config).expect("Graph client");
    let identity = client.drive_identity(&token).expect("drive identity");
    let expected = std::env::var("MEGLE_ONEDRIVE_EXPECT_ACCOUNT_KIND").ok();
    assert!(matches!(
        identity.account_kind.as_str(),
        "personal" | "business"
    ));
    if let Some(expected) = expected {
        assert_eq!(identity.account_kind, expected);
    }

    // Every release run gets a fresh namespace so immutable-object semantics
    // are tested without deleting or mutating prior evidence.
    let namespace = uuid::Uuid::now_v7().to_string();
    let immutable_key = format!("canary-immutable-{}.bin", uuid::Uuid::now_v7());
    let immutable_bytes = b"megle-live-onedrive-immutable-range-canary";
    let immutable_hash = hex(&Sha256::digest(immutable_bytes));

    let initial_delta = client
        .delta_list(&token, Some(&namespace), None, None, 2)
        .expect("initial namespace Delta and folder creation");
    assert!(initial_delta["objects"].as_array().is_some());
    let created = client
        .put_immutable(
            &token,
            &namespace,
            &immutable_key,
            immutable_bytes,
            &immutable_hash,
        )
        .expect("immutable put");
    assert_eq!(created["status"], "created");
    let repeated = client
        .put_immutable(
            &token,
            &namespace,
            &immutable_key,
            immutable_bytes,
            &immutable_hash,
        )
        .expect("idempotent immutable put");
    assert_eq!(repeated["status"], "already_exists_matching");

    let range_start = 6_u64;
    let range_end = 25_u64;
    let mut range = client
        .download(
            &token,
            &namespace,
            &immutable_key,
            Some((range_start, range_end)),
            immutable_bytes.len() as u64,
        )
        .expect("ranged download");
    assert_eq!(range.response_start, range_start);
    assert_eq!(range.response_end, range_end);
    assert!(
        !range.whole_object,
        "Graph must honor the canary Range request"
    );
    let mut range_bytes = Vec::new();
    range.file.read_to_end(&mut range_bytes).unwrap();
    assert_eq!(
        range_bytes,
        immutable_bytes[range_start as usize..range_end as usize]
    );

    let upload_key = format!("canary-upload-{}.bin", uuid::Uuid::now_v7());
    let upload_bytes = vec![0x5a; 512 * 1024];
    let upload_hash = hex(&Sha256::digest(&upload_bytes));
    let mut upload = client
        .begin_upload(
            &token,
            &namespace,
            &upload_key,
            upload_bytes.len() as u64,
            &upload_hash,
        )
        .expect("begin upload session");
    assert_eq!(
        client.upload_status(&upload).expect("upload status"),
        Some(0)
    );
    let committed = client
        .upload_fragment(&mut upload, 0, &upload_bytes)
        .expect("commit upload session");
    assert_eq!(committed, upload_bytes.len() as u64);
    let uploaded = client
        .head(&token, &namespace, &upload_key)
        .expect("uploaded head")
        .expect("uploaded object exists");
    assert_eq!(uploaded.size, upload_bytes.len() as u64);
    assert_eq!(uploaded.sha256, upload_hash);

    let mut page_token = None;
    let mut cursor = None;
    let mut delta_keys = Vec::new();
    loop {
        let page = client
            .delta_list(
                &token,
                Some(&namespace),
                cursor.as_deref(),
                page_token.as_deref(),
                1,
            )
            .expect("paged Delta");
        delta_keys.extend(
            page["objects"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|object| object["key"].as_str().map(str::to_owned)),
        );
        page_token = page["pageToken"].as_str().map(str::to_owned);
        if page_token.is_none() {
            cursor = page["completedCursor"].as_str().map(str::to_owned);
            break;
        }
    }
    assert!(cursor.is_some(), "Delta must return a completed cursor");
    assert!(delta_keys.contains(&immutable_key));
    assert!(delta_keys.contains(&upload_key));
    let unchanged = client
        .delta_list(&token, Some(&namespace), cursor.as_deref(), None, 10)
        .expect("cursor Delta");
    assert!(unchanged["completedCursor"].as_str().is_some());

    eprintln!(
        "ONEDRIVE_CANARY_JSON={}",
        serde_json::json!({
            "accountKind": identity.account_kind,
            "delta": true,
            "immutable": true,
            "range": true,
            "uploadSession": true,
            "namespaceHint": &hex(&Sha256::digest(namespace.as_bytes()))[..16],
        })
    );
}
