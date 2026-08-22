use uuid::Uuid;

use crate::error::ProviderError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryKey {
    pub library_uuid: Uuid,
    pub relative_key: String,
}

pub fn parse_library_key(namespace: &str, key: &str) -> Result<LibraryKey, ProviderError> {
    let library_uuid = Uuid::parse_str(namespace)
        .map_err(|_| ProviderError::new("invalid_argument", false, "invalid Library namespace"))?;
    if library_uuid.to_string() != namespace {
        return Err(ProviderError::new(
            "invalid_argument",
            false,
            "Library namespace is not canonical",
        ));
    }
    validate_relative_key(key)?;
    Ok(LibraryKey {
        library_uuid,
        relative_key: key.to_owned(),
    })
}

pub fn validate_relative_key(key: &str) -> Result<(), ProviderError> {
    if key.is_empty()
        || key.len() > 2048
        || key.starts_with('/')
        || key.ends_with('/')
        || key.contains('\\')
        || key.contains('%')
        || key.chars().any(|character| {
            character.is_control()
                || !(character.is_ascii_alphanumeric()
                    || matches!(character, '/' | '-' | '_' | '.'))
        })
        || key
            .split('/')
            .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
    {
        return Err(ProviderError::new(
            "invalid_argument",
            false,
            "invalid canonical object key",
        ));
    }
    Ok(())
}

pub fn graph_path(namespace: &str, key: &str) -> Result<String, ProviderError> {
    let parsed = parse_library_key(namespace, key)?;
    Ok(format!(
        "Megle/libraries/{}/{}",
        parsed.library_uuid, parsed.relative_key
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_keys_cannot_escape_library_namespace() {
        let namespace = "018f0000-0000-7000-8000-000000000001";
        assert_eq!(
            graph_path(namespace, "media/data-packs/ab/hash.pack").unwrap(),
            format!("Megle/libraries/{namespace}/media/data-packs/ab/hash.pack")
        );
        for invalid in [
            "", "/a", "a/", "a//b", "../a", "a\\b", "a%2fb", "a?b", "a#b", "a:b", "a\nb",
        ] {
            assert!(graph_path(namespace, invalid).is_err(), "{invalid}");
        }
        assert!(graph_path("018F0000-0000-7000-8000-000000000001", "segments/a").is_err());
    }
}
