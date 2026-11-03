//! Actual standalone process lifecycle; no provider key, database or running Core needed.
use serde_json::json;
use std::{
    fs::{DirBuilder, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

struct Fixture {
    directory: PathBuf,
    child: Option<Child>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child
            && child.try_wait().ok().flatten().is_none()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
fn private_file(path: &Path, bytes: &[u8]) {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap()
        .write_all(bytes)
        .unwrap();
}

#[tokio::test]
async fn sigterm_stops_the_real_runtime_and_removes_its_owned_socket() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "spitzli-neon-process-{}-{nonce}",
        std::process::id()
    ));
    DirBuilder::new().mode(0o700).create(&directory).unwrap();
    let mut fixture = Fixture {
        directory,
        child: None,
    };
    let manifest = fixture.directory.join("manifest.json");
    private_file(&manifest, include_bytes!("../manifest.json"));
    let socket = fixture.directory.join("runtime.sock");
    let configuration = fixture.directory.join("runtime.json");
    let issuer = ed25519_dalek::SigningKey::from_bytes(&[77; 32]);
    private_file(
        &configuration,
        &serde_json::to_vec(&json!({
            "manifest":manifest,"issuerPublicKeyHex":hex::encode(issuer.verifying_key().to_bytes()),
            "generation":1,"listen":socket,"core":fixture.directory.join("absent-core.sock"),
            "corePeers":[{"kind":"unix","uid":rustix::process::geteuid().as_raw()}]
        }))
        .unwrap(),
    );
    fixture.child = Some(
        Command::new(env!("CARGO_BIN_EXE_webdock-neon"))
            .arg(configuration)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    tokio::time::timeout(Duration::from_secs(8), async {
        while !socket.exists() {
            assert!(
                fixture
                    .child
                    .as_mut()
                    .unwrap()
                    .try_wait()
                    .unwrap()
                    .is_none(),
                "runtime exited before binding"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    // A response (including unavailable Core authority) proves the server loop is
    // running and its shutdown future has installed the termination handler.
    let mut client = webdock_plugin_runtime::connect_unix(&socket).await.unwrap();
    let _ = tokio::time::timeout(
        Duration::from_secs(5),
        client.health(webdock_plugin_runtime::wire::HealthRequest {}),
    )
    .await
    .unwrap();
    let child = fixture.child.as_mut().unwrap();
    let pid = rustix::process::Pid::from_raw(i32::try_from(child.id()).unwrap()).unwrap();
    rustix::process::kill_process(pid, rustix::process::Signal::TERM).unwrap();
    let status = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(status.success(), "runtime did not shut down gracefully");
    assert!(
        !socket.exists(),
        "owned runtime socket remained after shutdown"
    );
}
