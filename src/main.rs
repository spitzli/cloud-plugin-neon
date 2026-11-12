//! Stateless native v2 Neon runtime over Core-admitted Unix sockets or pinned mTLS.
use serde::Deserialize;
use spitzli_plugin_kit::transport::{CoreTls, ListenTls, RuntimeTransport};
use spitzli_plugin_runtime::{self as runtime};
use std::{path::PathBuf, process::ExitCode, sync::Arc};
type Error = Box<dyn std::error::Error + Send + Sync>;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Configuration {
    manifest: PathBuf,
    issuer_public_key_hex: String,
    generation: u64,
    listen: Option<PathBuf>,
    core: Option<PathBuf>,
    listen_tls: Option<ListenTls>,
    core_tls: Option<CoreTls>,
    core_peers: Vec<runtime::CorePeer>,
}
fn configuration(path: &std::path::Path) -> Result<Configuration, Error> {
    let bytes = spitzli_plugin_kit::read_file(path, true)?;
    if bytes.len() > 262144 {
        return Err("configuration limit".into());
    }
    Ok(serde_json::from_slice(&bytes)?)
}
fn validate_transport(transport: &RuntimeTransport) -> Result<(), Error> {
    transport.validate()?;
    if transport.listen.is_some() != transport.core.is_some() {
        return Err("listener and callback transport must match".into());
    }
    Ok(())
}
fn issuer_key(value: &str) -> Result<ed25519_dalek::VerifyingKey, Error> {
    if value.len() != 64 {
        return Err("invalid issuer key".into());
    }
    let key: [u8; 32] = hex::decode(value)?
        .try_into()
        .map_err(|_| "invalid issuer key")?;
    Ok(ed25519_dalek::VerifyingKey::from_bytes(&key)?)
}
async fn shutdown() {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("termination signal handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {},
        _ = terminate.recv() => {},
    }
}
async fn run() -> Result<(), Error> {
    let mut args = std::env::args_os().skip(1);
    let path = PathBuf::from(args.next().ok_or("configuration path required")?);
    if args.next().is_some() {
        return Err("unexpected argument".into());
    }
    let config = configuration(&path)?;
    let transport = RuntimeTransport {
        listen: config.listen,
        core: config.core,
        listen_tls: config.listen_tls,
        core_tls: config.core_tls,
    };
    validate_transport(&transport)?;
    let key = issuer_key(&config.issuer_public_key_hex)?;
    let manifest = spitzli_plugin_protocol::Manifest::parse(&spitzli_plugin_kit::read_file(
        &config.manifest,
        false,
    )?)?;
    spitzli_neon::validate_manifest(&manifest)?;
    let client = transport.core_client_lazy().await?;
    let authority = runtime::RemoteAuthority::new(
        client.clone(),
        manifest.id().into(),
        config.generation,
        config.core_peers,
    )?;
    let runtime = runtime::Runtime::new(
        manifest,
        config.generation,
        key,
        Arc::new(authority),
        Arc::new(spitzli_neon::Neon::new(client)),
    )?;
    match (&transport.listen, &transport.listen_tls) {
        (Some(path), None) => {
            runtime
                .serve_unix(runtime::UnixSocket::bind(path)?, shutdown())
                .await?
        }
        (None, Some(tls)) => {
            let material = tls.material()?;
            runtime
                .serve_mtls(
                    tokio::net::TcpListener::bind(tls.address).await?,
                    material,
                    shutdown(),
                )
                .await?
        }
        _ => return Err("listener required".into()),
    }
    Ok(())
}
#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => {
            eprintln!("neon_start_failed");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

    fn managed() -> Value {
        json!({
            "manifest":"/run/webdock/runtime/manifest.json",
            "issuerPublicKeyHex":hex::encode(ed25519_dalek::SigningKey::from_bytes(&[7; 32]).verifying_key().to_bytes()),
            "generation":2,
            "listenTls":{"address":"0.0.0.0:8443","certificate":"/run/webdock/runtime/runtime.crt","privateKey":"/run/webdock/runtime/runtime.key","authority":"/run/webdock/runtime/core-ca.crt"},
            "coreTls":{"origin":"https://core.test:8443","domain":"core.test","certificate":"/run/webdock/runtime/runtime.crt","privateKey":"/run/webdock/runtime/runtime.key","authority":"/run/webdock/runtime/core-ca.crt","serverCertificateSha256":"a".repeat(64)},
            "corePeers":[{"kind":"mutualTls","certificateSha256":"b".repeat(64)}]
        })
    }
    fn transport(value: Value) -> Result<RuntimeTransport, Error> {
        let config: Configuration = serde_json::from_value(value)?;
        issuer_key(&config.issuer_public_key_hex)?;
        let transport = RuntimeTransport {
            listen: config.listen,
            core: config.core,
            listen_tls: config.listen_tls,
            core_tls: config.core_tls,
        };
        validate_transport(&transport)?;
        Ok(transport)
    }
    #[test]
    fn worker_managed_shape_and_legacy_unix_need_no_provider_or_storage_configuration() {
        assert!(transport(managed()).is_ok());
        let mut unix = managed();
        unix.as_object_mut().unwrap().remove("listenTls");
        unix.as_object_mut().unwrap().remove("coreTls");
        unix["listen"] = json!("/run/webdock/runtime/plugin.sock");
        unix["core"] = json!("/run/webdock/runtime/core.sock");
        unix["corePeers"] = json!([{"kind":"unix","uid":1000}]);
        assert!(transport(unix).is_ok());
    }
    #[test]
    fn unsafe_mixed_and_unpinned_transports_are_rejected() {
        for (field, value) in [
            ("origin", "http://core.test:8443"),
            ("origin", "https://core.test/path"),
            ("origin", "https://user:secret@core.test"),
            ("domain", "other.test"),
            ("serverCertificateSha256", ""),
            ("serverCertificateSha256", "z"),
            ("privateKey", "relative.key"),
        ] {
            let mut bad = managed();
            bad["coreTls"][field] = json!(value);
            assert!(transport(bad).is_err(), "accepted {field}");
        }
        let mut absent_pin = managed();
        absent_pin["coreTls"]
            .as_object_mut()
            .unwrap()
            .remove("serverCertificateSha256");
        assert!(transport(absent_pin).is_err());
        for field in ["listen", "core"] {
            let mut duplicate = managed();
            duplicate[field] = json!("/run/webdock/runtime/socket");
            assert!(transport(duplicate).is_err());
            let mut mixed = managed();
            mixed.as_object_mut().unwrap().remove(if field == "listen" {
                "listenTls"
            } else {
                "coreTls"
            });
            mixed[field] = json!("/run/webdock/runtime/socket");
            assert!(transport(mixed).is_err());
        }
    }
    #[test]
    fn issuer_and_unexpected_authority_configuration_are_rejected() {
        for key in ["short".to_owned(), "z".repeat(64)] {
            let mut bad = managed();
            bad["issuerPublicKeyHex"] = json!(key);
            assert!(transport(bad).is_err());
        }
        for field in [
            "sourceKey",
            "providerUrl",
            "apiKey",
            "storageUrl",
            "storageCa",
            "secretKeyHex",
            "metadataDirectory",
        ] {
            let mut bad = managed();
            bad[field] = json!("not-accepted");
            assert!(transport(bad).is_err(), "accepted {field}");
        }
        for field in ["listenTls", "coreTls"] {
            let mut bad = managed();
            bad[field]["apiKey"] = json!("not-accepted");
            assert!(transport(bad).is_err());
        }
    }
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "neon-startup-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&path)
                .unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    #[tokio::test]
    async fn authority_rejects_empty_or_malformed_core_pins_and_wrong_generations() {
        let fixture = Fixture::new();
        let client = runtime::HostClient::new(
            runtime::unix_channel_lazy(fixture.0.join("core.sock")).unwrap(),
        );
        for (generation, peers) in [
            (2, vec![]),
            (0, vec![runtime::CorePeer::Unix { uid: 1000 }]),
            (
                2,
                vec![runtime::CorePeer::MutualTls {
                    certificate_sha256: "z".repeat(64),
                }],
            ),
        ] {
            assert!(
                runtime::RemoteAuthority::new(client.clone(), "neon".into(), generation, peers)
                    .is_err()
            );
        }
        assert!(
            runtime::RemoteAuthority::new(
                client,
                "neon".into(),
                2,
                vec![runtime::CorePeer::MutualTls {
                    certificate_sha256: "a".repeat(64)
                }],
            )
            .is_ok()
        );
    }
    #[test]
    fn startup_configuration_is_private_owned_bounded_and_not_a_link() {
        use std::io::Write;
        let fixture = Fixture::new();
        let path = fixture.0.join("runtime.json");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        file.write_all(&serde_json::to_vec(&managed()).unwrap())
            .unwrap();
        assert!(configuration(&path).is_ok());
        let symlink = fixture.0.join("link.json");
        std::os::unix::fs::symlink(&path, &symlink).unwrap();
        assert!(configuration(&symlink).is_err());
        let hardlink = fixture.0.join("hardlink.json");
        std::fs::hard_link(&path, &hardlink).unwrap();
        assert!(configuration(&path).is_err());
        std::fs::remove_file(hardlink).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(configuration(&path).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        file.set_len(262145).unwrap();
        assert!(configuration(&path).is_err());
        assert!(configuration(&fixture.0).is_err());
        assert!(configuration(std::path::Path::new("relative.json")).is_err());
    }
}
