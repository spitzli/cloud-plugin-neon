use std::io::{self, Read, Write};
use spitzli_plugin_protocol::Manifest;

const MAX_REGISTRY_BYTES: usize = 64 * 1024;

fn validate(bytes: &[u8], authoring: bool) -> Result<(), &'static str> {
    if bytes.len() > MAX_REGISTRY_BYTES {
        return Err("Manifest exceeds registry 64 KiB limit");
    }
    let manifest = Manifest::parse(bytes).map_err(|_| "Invalid native-v2 manifest")?;
    if !authoring
        && manifest.as_json()["image"]
            .as_str()
            .is_none_or(|image| image.ends_with(&format!("@sha256:{}", "0".repeat(64))))
    {
        return Err("Nonzero immutable image digest required");
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let authoring = match arguments.as_slice() {
        [] => false,
        [argument] if argument == "--authoring" => true,
        _ => return Err("Usage: manifest-validator [--authoring] < manifest.json".into()),
    };
    let mut bytes = Vec::new();
    io::stdin()
        .take((MAX_REGISTRY_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    validate(&bytes, authoring)?;
    // Preserve the exact release bytes; never normalize a signed descriptor.
    io::stdout().write_all(&bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    fn fixture() -> Value {
        serde_json::from_str(include_str!("../../../manifest.json")).unwrap()
    }
    #[test]
    fn authoring_and_release_require_native_manifest_and_final_digest() {
        let mut value = fixture();
        assert!(validate(&serde_json::to_vec(&value).unwrap(), true).is_ok());
        assert!(validate(&serde_json::to_vec(&value).unwrap(), false).is_err());
        value["image"] = json!(format!(
            "ghcr.io/spitzli/cloud-plugin-neon@sha256:{}",
            "a".repeat(64)
        ));
        assert!(validate(&serde_json::to_vec(&value).unwrap(), false).is_ok());
        value["capabilities"] = json!(["plugin.root"]);
        assert!(validate(&serde_json::to_vec(&value).unwrap(), true).is_err());
    }
}
