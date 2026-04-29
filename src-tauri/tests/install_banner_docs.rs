use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

#[test]
fn install_completion_uses_brand_banner_and_legacy_command() {
    let root = repo_root();
    let banner = fs::read_to_string(root.join("assets/brand/cli-banner.txt"))
        .expect("brand CLI banner should be readable");
    let install_sh =
        fs::read_to_string(root.join("scripts/install.sh")).expect("install.sh should be readable");
    let install_ps1 = fs::read_to_string(root.join("scripts/install.ps1"))
        .expect("install.ps1 should be readable");

    for line in banner.lines().filter(|line| !line.trim().is_empty()) {
        assert!(
            install_sh.contains(line),
            "install.sh completion banner should include {line:?}"
        );
        assert!(
            install_ps1.contains(line),
            "install.ps1 completion banner should include {line:?}"
        );
    }

    assert!(
        install_sh.contains("BINARY_NAME=\"oneshim\"")
            && install_sh.contains("Run command: $BINARY_NAME"),
        "macOS/Linux installer should keep the compatible CLI command"
    );
    assert!(
        install_ps1.contains("run: oneshim"),
        "Windows installer should keep the compatible CLI command"
    );
}
