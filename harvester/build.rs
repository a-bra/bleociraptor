// ABOUTME: Firmware build script — ESP-IDF glue, a friendly error when the
// ABOUTME: gitignored devices.rs is missing, and the git describe for build_info.

use std::path::Path;
use std::process::Command;

fn main() {
    // devices.rs is gitignored (§5.3); without this check a fresh clone fails
    // with an unresolved-module error that explains nothing.
    if !Path::new("src/devices.rs").exists() {
        panic!(
            "\n\nharvester/src/devices.rs is missing.\n\
             Copy src/devices.rs.example to src/devices.rs and fill in your \
             Wi-Fi credentials, OTA token and device list. The file is \
             gitignored and must never be committed (SPEC.md §5.3).\n\n"
        );
    }

    // `--dirty` is load-bearing (§13): without it a dirty tree reports the last
    // commit's sha and push_ota.sh's verification passes while the device runs
    // uncommitted code.
    let describe = Command::new("git")
        .args(["describe", "--always", "--dirty"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=HARVESTER_GIT_SHA={describe}");
    println!("cargo:rerun-if-changed=../.git/HEAD");

    // §18: the dashboard is gzipped at BUILD time and served from flash;
    // never assemble the page in RAM per request.
    gzip_dashboard();

    embuild::espidf::sysenv::output();
}

fn gzip_dashboard() {
    use flate2::{write::GzEncoder, Compression};
    use std::io::Write as _;

    println!("cargo:rerun-if-changed=assets/index.html");
    let html = std::fs::read("assets/index.html").expect("assets/index.html is part of the tree");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"))
        .join("index.html.gz");
    let mut enc = GzEncoder::new(Vec::new(), Compression::best());
    enc.write_all(&html).expect("gzip write");
    let gz = enc.finish().expect("gzip finish");
    std::fs::write(&out, &gz).expect("write gzipped dashboard");
    println!(
        "cargo:warning=dashboard: {} bytes -> {} gzipped",
        html.len(),
        gz.len()
    );
}
