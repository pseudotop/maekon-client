use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

#[test]
fn gitignore_covers_tauri_generated_sidecar_binaries() {
    let root = repo_root();
    let gitignore = fs::read_to_string(root.join(".gitignore")).expect(".gitignore is readable");

    assert!(
        gitignore
            .lines()
            .any(|line| line.trim() == "src-tauri/binaries/oneshim-sandbox-worker*"),
        ".gitignore should ignore Tauri-generated sandbox worker sidecars under src-tauri/binaries/"
    );
}

#[test]
fn release_reliability_smoke_can_require_signature_verification() {
    let root = repo_root();
    let script = fs::read_to_string(root.join("scripts/release-reliability-smoke.sh"))
        .expect("release reliability smoke script is readable");

    assert!(
        script.contains("ONESHIM_SMOKE_REQUIRE_SIGNATURE"),
        "release smoke should expose an env override for requiring signatures"
    );
    assert!(
        script.contains("--require-signature"),
        "release smoke should document and pass through --require-signature"
    );
    assert!(
        script.contains("SIGNATURE_PATH=\"$ARTIFACT_PATH.sig\""),
        "release smoke should resolve the expected signature sidecar path"
    );
    assert!(
        script.contains("[[ -f \"$SIGNATURE_PATH\" ]] || fatal"),
        "release smoke should fail early when signature verification is required but the sidecar is missing"
    );
    assert!(
        script.contains("INSTALL_ARGS+=(--require-signature)"),
        "release smoke should invoke the installer in fail-closed signature mode"
    );
}

#[test]
fn release_workflow_runs_signed_installer_smoke_before_publishing() {
    let root = repo_root();
    let workflow = fs::read_to_string(root.join(".github/workflows/release.yml"))
        .expect("release workflow is readable");

    let sign_step = workflow
        .find("Sign release artifacts (Ed25519)")
        .expect("release workflow should sign artifacts");
    let smoke_step = workflow
        .find("Run signed release reliability smoke")
        .expect("release workflow should smoke signed installer verification");

    assert!(
        sign_step < smoke_step,
        "signed release smoke should run after Ed25519 signatures are generated"
    );
    assert!(
        workflow.contains(
            "./scripts/release-reliability-smoke.sh --assets-dir dist --asset-name oneshim-linux-x64.tar.gz --skip-updater-tests --require-signature"
        ),
        "release workflow should run installer smoke in fail-closed signature mode before publishing"
    );
}

#[test]
fn release_archives_and_macos_app_bundle_include_sandbox_worker_sidecar() {
    let root = repo_root();
    let workflow = fs::read_to_string(root.join(".github/workflows/release.yml"))
        .expect("release workflow is readable");

    assert!(
        workflow.contains(
            r#"tar -czvf ../../../dist/"$ARTIFACT_NAME"."$ASSET_EXT" oneshim oneshim-sandbox-worker icon.icns"#
        ),
        "macOS per-architecture release archives should include the sandbox worker sidecar"
    );
    assert!(
        workflow.contains(
            r#"tar -czvf ../../../dist/"$ARTIFACT_NAME"."$ASSET_EXT" oneshim oneshim-sandbox-worker"#
        ),
        "Linux release archives should include the sandbox worker sidecar"
    );
    assert!(
        workflow.contains(
            r#"Compress-Archive -Path target/${{ matrix.target }}/release/oneshim.exe,target/${{ matrix.target }}/release/oneshim-sandbox-worker.exe"#
        ),
        "Windows release archives should include the sandbox worker sidecar"
    );
    assert!(
        workflow.contains("mv binaries/oneshim-sandbox-worker binaries/oneshim-sandbox-worker-arm64")
            && workflow
                .contains("mv binaries/oneshim-sandbox-worker binaries/oneshim-sandbox-worker-x64")
            && workflow.contains(
                "lipo -create binaries/oneshim-sandbox-worker-arm64 binaries/oneshim-sandbox-worker-x64 -output binaries/oneshim-sandbox-worker",
            ),
        "macOS universal packaging should merge the sandbox worker sidecar"
    );
    assert!(
        workflow.contains(
            "tar -czvf dist/oneshim-macos-universal.tar.gz -C binaries oneshim oneshim-sandbox-worker icon.icns",
        ),
        "macOS universal installer archive should include the sandbox worker sidecar"
    );
    assert!(
        workflow.contains(r#"cp binaries/oneshim-sandbox-worker "$APP_BUNDLE/Contents/MacOS/oneshim-sandbox-worker""#),
        "the hand-built macOS app bundle should include the sandbox worker sidecar"
    );
}

#[test]
fn installers_copy_and_smoke_check_sandbox_worker_sidecar() {
    let root = repo_root();
    let install_sh =
        fs::read_to_string(root.join("scripts/install.sh")).expect("install.sh is readable");
    let install_ps1 =
        fs::read_to_string(root.join("scripts/install.ps1")).expect("install.ps1 is readable");
    let smoke_sh = fs::read_to_string(root.join("scripts/release-reliability-smoke.sh"))
        .expect("release reliability smoke script is readable");
    let macos_installer_smoke =
        fs::read_to_string(root.join("scripts/release-installer-smoke-macos.sh"))
            .expect("macOS installer smoke script is readable");

    assert!(
        install_sh.contains(r#"SIDECAR_NAME="oneshim-sandbox-worker""#)
            && install_sh.contains(r#"install_sidecar_if_present "$APP_BUNDLE/Contents/MacOS""#)
            && install_sh.contains(r#"install_sidecar_if_present "$INSTALL_DIR""#),
        "install.sh should install the sandbox worker beside the app/binary when present"
    );
    assert!(
        install_ps1.contains(r#"$SidecarName = "oneshim-sandbox-worker.exe""#)
            && install_ps1.contains("$sidecar = Get-ChildItem")
            && install_ps1.contains("$sidecarTarget = Join-Path $InstallDir $SidecarName"),
        "install.ps1 should install the Windows sandbox worker sidecar when present"
    );
    assert!(
        smoke_sh.contains(r#"TARGET_SIDECAR="$INSTALL_DIR/oneshim-sandbox-worker""#)
            && smoke_sh
                .contains(r#"APP_SIDECAR="$APP_BUNDLE/Contents/MacOS/oneshim-sandbox-worker""#),
        "release reliability smoke should fail if the installer drops the sandbox worker sidecar"
    );
    assert!(
        macos_installer_smoke.contains(r#"DMG_SIDECAR_PATH="$DMG_APP_PATH/Contents/MacOS/oneshim-sandbox-worker""#)
            && macos_installer_smoke.contains(r#"APP_SIDECAR_PATH="$APP_INSTALL_PATH/Contents/MacOS/oneshim-sandbox-worker""#),
        "macOS installer smoke should verify DMG and PKG app bundles include the sandbox worker sidecar"
    );
}

#[test]
fn pkg_builder_supports_unsigned_builds_with_strict_shell_options() {
    let root = repo_root();
    let script = fs::read_to_string(root.join("src-tauri/pkg/build-pkg.sh"))
        .expect("PKG builder script is readable");

    assert!(
        script.contains("build_product_archive()"),
        "PKG builder should wrap productbuild so signed and unsigned invocations do not rely on an empty array"
    );
    assert!(
        !script.contains(r#""${SIGN_ARGS[@]}""#),
        "PKG builder should not expand an empty SIGN_ARGS array under set -u"
    );
}
