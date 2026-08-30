use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn asset_contains_maekon_brand_color(path: &Path) -> bool {
    let bytes = fs::read(path).unwrap_or_else(|err| {
        panic!(
            "installer branding asset should be readable: {} ({err})",
            path.display()
        )
    });
    let brand_rgb = [
        [0x15, 0x04, 0x3a],
        [0x55, 0x34, 0xd7],
        [0x61, 0xf4, 0xd6],
        [0x0d, 0xe0, 0xa2],
    ];

    brand_rgb.iter().any(|rgb| {
        let bgr = [rgb[2], rgb[1], rgb[0]];
        bytes
            .windows(3)
            .any(|window| window == rgb.as_slice() || window == bgr.as_slice())
    })
}

#[test]
fn gitignore_covers_tauri_generated_sidecar_binaries() {
    let root = repo_root();
    let gitignore = fs::read_to_string(root.join(".gitignore")).expect(".gitignore is readable");

    assert!(
        gitignore
            .lines()
            .any(|line| line.trim() == "src-tauri/binaries/maekon-sandbox-worker*"),
        ".gitignore should ignore Tauri-generated sandbox worker sidecars under src-tauri/binaries/"
    );
}

#[test]
fn release_reliability_smoke_can_require_signature_verification() {
    let root = repo_root();
    let script = fs::read_to_string(root.join("scripts/release-reliability-smoke.sh"))
        .expect("release reliability smoke script is readable");

    assert!(
        script.contains("MAEKON_SMOKE_REQUIRE_SIGNATURE"),
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
            "./scripts/release-reliability-smoke.sh --assets-dir dist --asset-name maekon-linux-x64.tar.gz --skip-updater-tests --require-signature"
        ),
        "release workflow should run installer smoke in fail-closed signature mode before publishing"
    );
}

#[test]
fn release_notes_quick_install_commands_are_pinned_to_release_tag() {
    let root = repo_root();
    let workflow = fs::read_to_string(root.join(".github/workflows/release.yml"))
        .expect("release workflow is readable");

    assert!(
        !workflow
            .contains("raw.githubusercontent.com/${{ github.repository }}/main/scripts/install.sh")
            && !workflow.contains(
                "raw.githubusercontent.com/${{ github.repository }}/main/scripts/install.ps1"
            ),
        "release note quick-install commands must not fetch mutable main-branch installer scripts"
    );
    assert!(
        workflow.contains(
            "raw.githubusercontent.com/${{ github.repository }}/${VERSION}/scripts/install.sh"
        ) && workflow.contains(
            "raw.githubusercontent.com/${{ github.repository }}/${VERSION}/scripts/install.ps1",
        ),
        "release note quick-install commands should fetch installer scripts from the release tag"
    );
    assert!(
        workflow
            .matches("MAEKON_VERSION=${VERSION} bash /tmp/maekon-install.sh --require-signature")
            .count()
            >= 2
            && workflow
                .matches("-Version ${VERSION} -RequireSignature")
                .count()
                >= 2,
        "both prerelease and stable quick-install commands should pin the artifact version"
    );
}

#[test]
fn ci_transparency_documents_local_signed_stable_tag_flow() {
    let root = repo_root();
    let docs = fs::read_to_string(root.join("docs/guides/ci-transparency.md"))
        .expect("CI transparency guide is readable");

    assert!(
        docs.contains("./scripts/publish-stable-tag.sh <x.y.z>"),
        "CI transparency guide should tell maintainers to publish the stable tag with publish-stable-tag.sh"
    );
    assert!(
        !docs.contains("let GitHub Actions create the stable tag"),
        "promote-stable.yml should be documented as opening the promotion PR, not creating the signed stable tag"
    );
    assert!(
        !docs.contains("maintainers do not push `vX.Y.Z` manually"),
        "stable tag publication should be described as a maintainer-local signed-tag script flow"
    );
}

#[test]
fn config_sync_require_artifacts_rejects_frontend_dist_without_js_bundle() {
    let root = repo_root();
    let script = fs::read_to_string(root.join("scripts/check-config-sync.sh"))
        .expect("config sync script is readable");
    let docs = fs::read_to_string(root.join("docs/testing/source-build-prerequisites.md"))
        .expect("source build prerequisites guide is readable");

    assert!(
        script.contains("[ \"$REQUIRE_ARTIFACTS\" -eq 1 ] && [ \"$JS_COUNT\" -eq 0 ]"),
        "--require-artifacts should reject placeholder dist/index.html without a JavaScript bundle"
    );
    assert!(
        script.contains("Frontend dist/ has no JavaScript artifacts"),
        "config sync failure should explain that a real frontend build is required"
    );
    assert!(
        docs.contains("at least one generated JavaScript bundle"),
        "source build docs should document what --require-artifacts validates"
    );
}

#[test]
fn release_archives_and_macos_app_bundle_include_sandbox_worker_sidecar() {
    let root = repo_root();
    let workflow = fs::read_to_string(root.join(".github/workflows/release.yml"))
        .expect("release workflow is readable");

    assert!(
        workflow.contains(
            r#"tar -czvf ../../../dist/"$ARTIFACT_NAME"."$ASSET_EXT" maekon maekon-sandbox-worker icon.icns"#
        ),
        "macOS per-architecture release archives should include the sandbox worker sidecar"
    );
    assert!(
        workflow.contains(
            r#"tar -czvf ../../../dist/"$ARTIFACT_NAME"."$ASSET_EXT" maekon maekon-sandbox-worker"#
        ),
        "Linux release archives should include the sandbox worker sidecar"
    );
    assert!(
        workflow.contains(
            r#"Compress-Archive -Path target/${{ matrix.target }}/release/maekon.exe,target/${{ matrix.target }}/release/maekon-sandbox-worker.exe"#
        ),
        "Windows release archives should include the sandbox worker sidecar"
    );
    assert!(
        workflow.contains("mv binaries/maekon-sandbox-worker binaries/maekon-sandbox-worker-arm64")
            && workflow
                .contains("mv binaries/maekon-sandbox-worker binaries/maekon-sandbox-worker-x64")
            && workflow.contains(
                "lipo -create binaries/maekon-sandbox-worker-arm64 binaries/maekon-sandbox-worker-x64 -output binaries/maekon-sandbox-worker",
            ),
        "macOS universal packaging should merge the sandbox worker sidecar"
    );
    assert!(
        workflow.contains(
            "tar -czvf dist/maekon-macos-universal.tar.gz -C binaries maekon maekon-sandbox-worker icon.icns",
        ),
        "macOS universal installer archive should include the sandbox worker sidecar"
    );
    assert!(
        workflow.contains(r#"cp binaries/maekon-sandbox-worker "$APP_BUNDLE/Contents/MacOS/maekon-sandbox-worker""#),
        "the hand-built macOS app bundle should include the sandbox worker sidecar"
    );
}

#[test]
fn windows_msi_manifest_installs_sandbox_worker_sidecar() {
    let root = repo_root();
    let wix_manifest =
        fs::read_to_string(root.join("src-tauri/wix/main.wxs")).expect("WiX manifest is readable");

    assert!(
        wix_manifest.contains("Name='maekon-sandbox-worker.exe'")
            && wix_manifest
                .contains(r#"Source='$(var.CargoTargetBinDir)\maekon-sandbox-worker.exe'"#),
        "Windows MSI manifest should install the sandbox worker beside maekon.exe"
    );
}

#[test]
fn windows_installers_use_maekon_branding_assets() {
    let root = repo_root();
    let tauri_config = fs::read_to_string(root.join("src-tauri/tauri.conf.json"))
        .expect("Tauri config is readable");
    let wix_manifest =
        fs::read_to_string(root.join("src-tauri/wix/main.wxs")).expect("WiX manifest is readable");

    for expected in [
        r#""headerImage": "nsis/header.bmp""#,
        r#""sidebarImage": "nsis/sidebar.bmp""#,
        r#""installerIcon": "icons/icon.ico""#,
    ] {
        assert!(
            tauri_config.contains(expected),
            "NSIS installer should keep Maekon branding asset reference: {expected}"
        );
    }

    for expected in [
        "WixUIBannerBmp",
        "WixUIDialogBmp",
        "ARPPRODUCTICON",
        "icons\\icon.ico",
    ] {
        assert!(
            wix_manifest.contains(expected),
            "MSI installer should keep Maekon branding manifest reference: {expected}"
        );
    }

    for asset in [
        "src-tauri/nsis/header.bmp",
        "src-tauri/nsis/sidebar.bmp",
        "src-tauri/wix/banner.bmp",
        "src-tauri/wix/dialog.bmp",
    ] {
        let asset_path = root.join(asset);
        assert!(
            asset_path.exists(),
            "Windows installer branding asset should exist: {asset}"
        );
        assert!(
            asset_contains_maekon_brand_color(&asset_path),
            "Windows installer branding asset should contain Maekon brand colors: {asset}"
        );
    }
}

#[test]
fn release_workflow_publishes_windows_nsis_setup_exe() {
    let root = repo_root();
    let workflow = fs::read_to_string(root.join(".github/workflows/release.yml"))
        .expect("release workflow is readable");

    for expected in [
        "Install Tauri CLI",
        "Build NSIS setup installer",
        "tauri build",
        "--bundles nsis",
        "tauri-nsis-ci-config.json",
        "Upload NSIS setup artifact",
        "maekon-windows-x64-setup-exe",
        "*.exe",
    ] {
        assert!(
            workflow.contains(expected),
            "release workflow should publish the Windows NSIS setup exe: {expected}"
        );
    }
}

#[test]
fn release_reliability_smoke_runs_updater_regression_on_all_release_platforms() {
    let root = repo_root();
    let workflow = fs::read_to_string(root.join(".github/workflows/release.yml"))
        .expect("release workflow is readable");

    assert!(
        !workflow.contains("run_updater_tests: false"),
        "release reliability smoke should not silently skip updater regressions on macOS or Windows"
    );
    assert!(
        workflow.matches("run_updater_tests: true").count() >= 3,
        "linux, macOS, and Windows release reliability smoke entries should run updater tests"
    );
}

#[test]
fn release_smoke_builds_real_sandbox_worker_sidecar_for_tauri_external_bin() {
    let root = repo_root();
    let release_workflow = fs::read_to_string(root.join(".github/workflows/release.yml"))
        .expect("release workflow is readable");
    let release_smoke_workflow =
        fs::read_to_string(root.join(".github/workflows/release-smoke.yml"))
            .expect("release-smoke workflow is readable");

    for (name, workflow) in [
        ("release.yml", release_workflow.as_str()),
        ("release-smoke.yml", release_smoke_workflow.as_str()),
    ] {
        assert!(
            !workflow.contains("Create sandbox worker stub for Tauri externalBin"),
            "{name} must build the real sandbox worker sidecar instead of touching a stub"
        );
        assert!(
            workflow.contains("-p maekon-sandbox-worker"),
            "{name} must build maekon-sandbox-worker before Tauri externalBin validation"
        );
        assert!(
            workflow.contains("maekon-sandbox-worker-${TRIPLE}")
                || workflow.contains("maekon-sandbox-worker-${TARGET}"),
            "{name} must copy the built sidecar into Tauri's expected externalBin name"
        );
    }
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
        install_sh.contains(r#"SIDECAR_NAME="maekon-sandbox-worker""#)
            && install_sh.contains(r#"install_sidecar_if_present "$APP_BUNDLE/Contents/MacOS""#)
            && install_sh.contains(r#"install_sidecar_if_present "$INSTALL_DIR""#),
        "install.sh should install the sandbox worker beside the app/binary when present"
    );
    assert!(
        install_ps1.contains(r#"$SidecarName = "maekon-sandbox-worker.exe""#)
            && install_ps1.contains("$sidecar = Get-ChildItem")
            && install_ps1.contains("$sidecarTarget = Join-Path $InstallDir $SidecarName"),
        "install.ps1 should install the Windows sandbox worker sidecar when present"
    );
    assert!(
        smoke_sh.contains(r#"TARGET_SIDECAR="$INSTALL_DIR/maekon-sandbox-worker""#)
            && smoke_sh
                .contains(r#"APP_SIDECAR="$APP_BUNDLE/Contents/MacOS/maekon-sandbox-worker""#),
        "release reliability smoke should fail if the installer drops the sandbox worker sidecar"
    );
    assert!(
        macos_installer_smoke.contains(r#"DMG_SIDECAR_PATH="$DMG_APP_PATH/Contents/MacOS/maekon-sandbox-worker""#)
            && macos_installer_smoke.contains(r#"APP_SIDECAR_PATH="$APP_INSTALL_PATH/Contents/MacOS/maekon-sandbox-worker""#),
        "macOS installer smoke should verify DMG and PKG app bundles include the sandbox worker sidecar"
    );
}

#[test]
fn macos_release_verifies_final_app_bundles_and_uses_apple_build_versions() {
    let root = repo_root();
    let release = fs::read_to_string(root.join(".github/workflows/release.yml"))
        .expect("release workflow is readable");
    let notarize =
        fs::read_to_string(root.join(".github/workflows/notarize-macos-release-assets.yml"))
            .expect("notarization workflow is readable");
    let installer_smoke = fs::read_to_string(root.join("scripts/release-installer-smoke-macos.sh"))
        .expect("macOS installer smoke script is readable");
    let verifier = fs::read_to_string(root.join("scripts/verify-macos-app-bundle.sh"))
        .expect("macOS app bundle verifier is readable");

    assert!(
        release.contains("BUNDLE_BUILD_VERSION=\"$(python3 scripts/macos-bundle-version.py"),
        "release packaging should convert SemVer prereleases into Apple-compatible build versions"
    );
    assert!(
        release.matches("./scripts/verify-macos-app-bundle.sh").count() >= 2,
        "release packaging should verify both the signed staging app and the app copied into the DMG"
    );
    assert!(
        notarize.contains("./scripts/verify-macos-app-bundle.sh"),
        "notarization should re-verify the app inside the final stapled DMG"
    );
    assert!(
        installer_smoke
            .matches("$SCRIPT_DIR/verify-macos-app-bundle.sh")
            .count()
            >= 2,
        "installer smoke should verify app signatures from both DMG and PKG paths"
    );
    assert!(
        verifier.contains("Info.plist=not bound")
            && verifier.contains("invalid entitlements blob")
            && verifier.contains("--arch"),
        "bundle verifier should reject the three rc.10 failure signals"
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
