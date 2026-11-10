//! Exercises the actual signed-download, verification, extraction and full-bundle install pipeline.
use super::*;
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
};
#[derive(Clone)]
struct Fixture(self_update::Release);
impl self_update::ReleaseSource for Fixture {
    fn get_releases(&self) -> self_update::Result<Vec<self_update::Release>> {
        Ok(vec![self.0.clone()])
    }
}
#[test]
#[ignore = "Requires YAS_SIGNED_UPDATE_FIXTURE pointing to a real signed macOS release ZIP"]
fn signed_bundle_install_and_tamper_rejection() {
    let fixture =
        PathBuf::from(std::env::var("YAS_SIGNED_UPDATE_FIXTURE").expect("signed fixture path"));
    let bytes = fs::read(&fixture).unwrap();
    assert!(self_update::verify_signature(&fixture, &[KEY]).is_ok());
    let root = std::env::temp_dir().join(format!("yas-release-install-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let install = root.join("Yas Browser.app");
    fs::create_dir_all(install.join("Contents")).unwrap();
    fs::write(install.join("Contents/old-marker"), b"old app").unwrap();
    for tampered in [true, false] {
        let mut payload = bytes.clone();
        if tampered {
            payload[0] ^= 1;
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 8192];
            let _ = stream.read(&mut request);
            let _=write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",payload.len());
            let _ = stream.write_all(&payload);
        });
        let asset = asset_name(CURRENT);
        let release = self_update::Release::builder()
            .version(CURRENT)
            .asset(self_update::ReleaseAsset::new(
                &asset,
                format!("http://{address}/{asset}"),
            ))
            .build()
            .unwrap();
        let result = self_update::backends::custom::Update::configure()
            .source(Fixture(release))
            .bin_name("yas-macos")
            .target("aarch64-apple-darwin")
            .asset_identifier("ota")
            .current_version("0.0.0")
            .bundle_path_in_archive("Yas Browser.app")
            .bundle_install_path(&install)
            .verifying_keys([KEY])
            .verify_binary(verify_bundle)
            .no_confirm(true)
            .show_output(false)
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap()
            .update();
        server.join().unwrap();
        if tampered {
            assert!(result.is_err());
            assert_eq!(
                fs::read(install.join("Contents/old-marker")).unwrap(),
                b"old app"
            );
        } else {
            assert!(result.unwrap().is_updated());
            assert!(!install.join("Contents/old-marker").exists());
            assert!(install.join("Contents/MacOS/yas-macos").is_file());
            assert!(install.join("Contents/Info.plist").is_file());
            verify_bundle(&install).unwrap();
        }
    }
    fs::remove_dir_all(root).unwrap();
}
