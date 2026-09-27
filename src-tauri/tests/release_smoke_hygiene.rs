use std::fs;
use std::path::{Path, PathBuf};

#[path = "../build.rs"]
mod app_build_script;

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

// #12181: Exercise the actual build helper with owned Git metadata. These tests
// prove revision/watch coverage; Cargo freshness is a separate paired build.
#[cfg(test)]
mod git_watch {
    use super::{app_build_script, fs, Path, PathBuf};
    use std::collections::BTreeSet;
    use std::ffi::OsString;
    use std::process::{Command, ExitStatus, Output};

    struct Fixture {
        owner: tempfile::TempDir,
        repo: PathBuf,
        empty_config: PathBuf,
        empty_hooks: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let owner = tempfile::Builder::new()
                .prefix("maekon-git-watch-")
                .tempdir()
                .expect("create owned Git fixture");
            let repo = owner.path().join("repository 한글 with spaces");
            let empty_config = owner.path().join("empty-config");
            let empty_hooks = owner.path().join("empty-hooks");
            fs::create_dir(&repo).expect("create fixture repository");
            fs::create_dir(&empty_hooks).expect("create empty hooks/template directory");
            fs::write(&empty_config, "").expect("create empty Git config");
            Self {
                owner,
                repo,
                empty_config,
                empty_hooks,
            }
        }

        fn run(&self, cwd: &Path, args: &[&str]) -> Output {
            let owner = self
                .owner
                .path()
                .canonicalize()
                .expect("owned fixture root");
            assert!(cwd.canonicalize().expect("fixture cwd").starts_with(&owner));
            let mut command = Command::new("git");
            command.current_dir(cwd).env_clear();
            // Keep only executable/OS plumbing, never inherited Git overrides,
            // credentials, global/system configuration, or hook templates.
            for name in ["PATH", "PATHEXT", "SystemRoot", "WINDIR", "TEMP", "TMP"] {
                if let Some(value) = std::env::var_os(name) {
                    command.env(name, value);
                }
            }
            command
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", &self.empty_config)
                .env("GIT_CEILING_DIRECTORIES", &owner)
                .env("GIT_TERMINAL_PROMPT", "0")
                .args([
                    "-c",
                    "user.name=Maekon Fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                ])
                .args(["-c", "commit.gpgSign=false", "-c", "gc.auto=0"])
                .arg("-c")
                .arg(format!("core.hooksPath={}", self.empty_hooks.display()))
                .arg("-c")
                .arg(format!("init.templateDir={}", self.empty_hooks.display()))
                .args(args);
            command
                .output()
                .expect("Git must be available for the owned fixture")
        }

        fn git(&self, cwd: &Path, args: &[&str]) -> String {
            let output = self.run(cwd, args);
            assert!(
                output.status.success(),
                "fixture Git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout)
                .expect("fixture Git UTF-8")
                .trim()
                .to_owned()
        }

        fn init(&self) {
            self.git(
                &self.repo,
                &["init", "--quiet", "--initial-branch=watch-main"],
            );
        }

        fn package(&self, repo: &Path) -> PathBuf {
            let package = repo.join("clients/maekon-client/src-tauri");
            fs::create_dir_all(&package).expect("create nested package");
            package
        }

        fn commit(&self, repo: &Path, content: &str) -> String {
            fs::write(repo.join("owned.txt"), content).expect("write owned fixture input");
            self.git(repo, &["add", "--", "owned.txt"]);
            self.git(repo, &["commit", "--quiet", "--no-gpg-sign", "-m", content]);
            self.git(repo, &["rev-parse", "--short=9", "HEAD"])
        }

        fn git_path(&self, repo: &Path, name: &str) -> PathBuf {
            PathBuf::from(self.git(
                repo,
                &["rev-parse", "--path-format=absolute", "--git-path", name],
            ))
        }

        fn metadata(&self, package: &Path) -> (String, Vec<PathBuf>) {
            let (revision, paths) = app_build_script::git_build_metadata(package);
            let owner = self.owner.path().canonicalize().expect("fixture root");
            let mut canonical = BTreeSet::new();
            for path in &paths {
                assert!(
                    path.is_absolute(),
                    "watch must resolve from the package cwd: {path:?}"
                );
                let resolved = path.canonicalize().expect("no nonexistent Git watches");
                assert!(
                    resolved.starts_with(&owner),
                    "watch escaped owned fixture: {path:?}"
                );
                assert_ne!(path.file_name(), Some(std::ffi::OsStr::new("index")));
                assert!(canonical.insert(resolved), "duplicate watch: {path:?}");
            }
            (revision, paths)
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    enum WatchState {
        Missing,
        File(Vec<u8>),
        Directory(Vec<OsString>),
    }

    fn snapshot(paths: &[PathBuf]) -> Vec<WatchState> {
        paths
            .iter()
            .map(|path| {
                if !path.exists() {
                    WatchState::Missing
                } else if path.is_dir() {
                    let mut names = fs::read_dir(path)
                        .expect("watched directory")
                        .map(|entry| entry.expect("watched entry").file_name())
                        .collect::<Vec<_>>();
                    names.sort();
                    WatchState::Directory(names)
                } else {
                    WatchState::File(fs::read(path).expect("watched file"))
                }
            })
            .collect()
    }

    fn contains_watch(paths: &[PathBuf], expected: &Path) -> bool {
        let expected = expected.canonicalize().expect("expected watch exists");
        paths
            .iter()
            .any(|path| path.canonicalize().expect("watch exists") == expected)
    }

    #[test]
    fn nested_unicode_checkout_watches_ref_changes_without_index() {
        let fixture = Fixture::new();
        fixture.init();
        let first = fixture.commit(&fixture.repo, "first");
        let package = fixture.package(&fixture.repo);
        let (revision, paths) = fixture.metadata(&package);
        assert_eq!(revision, first);
        let head = fixture.git_path(&package, "HEAD");
        let reference = fixture.git_path(&package, "refs/heads/watch-main");
        assert!(contains_watch(&paths, &head));
        assert!(contains_watch(&paths, &reference));
        let before = snapshot(&paths);
        assert_eq!(
            fixture.metadata(&package),
            (revision.clone(), paths.clone())
        );
        assert_eq!(snapshot(&paths), before);
        fs::write(fixture.repo.join("owned.txt"), "staged-only").expect("stage-only fixture");
        fixture.git(&fixture.repo, &["add", "--", "owned.txt"]);
        assert_eq!(
            snapshot(&paths),
            before,
            "index-only update is not a revision input"
        );
        let head_before = fs::read(&head).expect("symbolic HEAD");
        let second = fixture.commit(&fixture.repo, "second");
        assert_ne!(first, second);
        assert_eq!(
            fs::read(&head).expect("unchanged symbolic HEAD"),
            head_before
        );
        assert_ne!(
            snapshot(&paths),
            before,
            "previous watches must observe the ref update"
        );
        assert_eq!(fixture.metadata(&package).0, second);
    }

    #[test]
    fn linked_worktree_uses_its_own_head_and_shared_ref() {
        let fixture = Fixture::new();
        fixture.init();
        let first = fixture.commit(&fixture.repo, "first");
        let linked = fixture.owner.path().join("linked 한글 worktree");
        fixture.git(
            &fixture.repo,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "linked-watch",
                linked.to_str().expect("fixture path"),
            ],
        );
        let package = fixture.package(&linked);
        let (revision, paths) = fixture.metadata(&package);
        assert_eq!(revision, first);
        assert!(linked.join(".git").is_file());
        assert!(contains_watch(&paths, &linked.join(".git")));
        let own_head = fixture.git_path(&package, "HEAD");
        let main_head = fixture.git_path(&fixture.repo, "HEAD");
        assert_ne!(own_head, main_head);
        assert!(contains_watch(&paths, &own_head));
        assert!(!contains_watch(&paths, &main_head));
        assert!(contains_watch(
            &paths,
            &fixture.git_path(&package, "refs/heads/linked-watch")
        ));
        assert!(contains_watch(
            &paths,
            &own_head
                .parent()
                .expect("worktree metadata")
                .join("commondir")
        ));
        let before = snapshot(&paths);
        let second = fixture.commit(&linked, "linked-second");
        assert_ne!(second, first);
        assert_ne!(snapshot(&paths), before);
        assert_eq!(fixture.metadata(&package).0, second);
        assert_eq!(
            fixture.git(&fixture.repo, &["rev-parse", "--short=9", "HEAD"]),
            first
        );
    }

    #[test]
    fn detached_head_change_is_observed() {
        let fixture = Fixture::new();
        fixture.init();
        let first = fixture.commit(&fixture.repo, "first");
        fixture.git(&fixture.repo, &["checkout", "--quiet", "--detach", "HEAD"]);
        let package = fixture.package(&fixture.repo);
        let (revision, paths) = fixture.metadata(&package);
        assert_eq!(revision, first);
        assert!(contains_watch(&paths, &fixture.git_path(&package, "HEAD")));
        let before = snapshot(&paths);
        let second = fixture.commit(&fixture.repo, "detached-second");
        assert_ne!(first, second);
        assert_ne!(snapshot(&paths), before);
        assert_eq!(fixture.metadata(&package).0, second);
    }

    #[test]
    fn packed_ref_to_new_loose_ref_is_observed_by_existing_parent() {
        let fixture = Fixture::new();
        fixture.init();
        let first = fixture.commit(&fixture.repo, "first");
        fixture.git(&fixture.repo, &["branch", "-m", "topic/watch"]);
        fixture.git(&fixture.repo, &["pack-refs", "--all", "--prune"]);
        let package = fixture.package(&fixture.repo);
        let reference = fixture.git_path(&package, "refs/heads/topic/watch");
        let packed = fixture.git_path(&package, "packed-refs");
        assert!(!reference.exists());
        assert!(packed.is_file());
        let mut ancestor = reference.parent().expect("ref parent");
        while !ancestor.exists() {
            ancestor = ancestor.parent().expect("existing refs ancestor");
        }
        let (revision, paths) = fixture.metadata(&package);
        assert_eq!(revision, first);
        assert!(contains_watch(&paths, ancestor));
        assert!(contains_watch(&paths, &packed));
        let head = fixture.git_path(&package, "HEAD");
        let head_before = fs::read(&head).expect("HEAD before loose ref");
        let packed_before = fs::read(&packed).expect("packed refs before loose ref");
        let before = snapshot(&paths);
        let second = fixture.commit(&fixture.repo, "new-loose-ref");
        assert_ne!(first, second);
        assert!(reference.is_file());
        assert_eq!(fs::read(&head).expect("same HEAD"), head_before);
        assert_eq!(fs::read(&packed).expect("same packed refs"), packed_before);
        assert_ne!(
            snapshot(&paths),
            before,
            "watch set must observe creation of the loose override"
        );
        let (revision, paths) = fixture.metadata(&package);
        assert_eq!(revision, second);
        assert!(contains_watch(&paths, &reference));
    }

    #[test]
    fn packing_a_watched_loose_ref_does_not_leave_missing_watches() {
        let fixture = Fixture::new();
        fixture.init();
        let revision = fixture.commit(&fixture.repo, "first");
        let package = fixture.package(&fixture.repo);
        let (_, old_paths) = fixture.metadata(&package);
        let before = snapshot(&old_paths);
        fixture.git(&fixture.repo, &["pack-refs", "--all", "--prune"]);
        assert_ne!(
            snapshot(&old_paths),
            before,
            "ref removal must invalidate the previous watch set"
        );
        let (after, paths) = fixture.metadata(&package);
        assert_eq!(after, revision);
        assert!(contains_watch(
            &paths,
            &fixture.git_path(&package, "packed-refs")
        ));
        assert_eq!(fixture.metadata(&package), (after, paths));
    }

    #[test]
    fn unborn_head_preserves_unknown_and_observes_first_commit() {
        let fixture = Fixture::new();
        fixture.init();
        let package = fixture.package(&fixture.repo);
        assert!(!fixture
            .run(&package, &["rev-parse", "--short=9", "HEAD"])
            .status
            .success());
        let (revision, paths) = fixture.metadata(&package);
        assert_eq!(revision, "unknown");
        assert!(contains_watch(&paths, &fixture.git_path(&package, "HEAD")));
        let before = snapshot(&paths);
        let first = fixture.commit(&fixture.repo, "first");
        assert_ne!(snapshot(&paths), before);
        assert_eq!(fixture.metadata(&package).0, first);
    }

    #[test]
    fn source_export_and_invalid_gitfile_return_unknown_without_missing_watches() {
        let fixture = Fixture::new();
        let package = fixture.package(&fixture.repo);
        assert!(!fixture
            .run(&package, &["rev-parse", "--short=9", "HEAD"])
            .status
            .success());
        assert_eq!(
            fixture.metadata(&package),
            ("unknown".to_owned(), Vec::new())
        );
        fs::write(
            fixture.repo.join(".git"),
            "gitdir: missing-owned-metadata\n",
        )
        .expect("invalid owned gitfile");
        assert_eq!(
            fixture.metadata(&package),
            ("unknown".to_owned(), Vec::new())
        );
        assert_eq!(
            fixture.metadata(&package.join("nonexistent")),
            ("unknown".to_owned(), Vec::new())
        );
    }

    #[test]
    fn failed_git_stdout_is_not_accepted_as_a_revision() {
        #[cfg(unix)]
        use std::os::unix::process::ExitStatusExt;
        #[cfg(windows)]
        use std::os::windows::process::ExitStatusExt;
        #[cfg(unix)]
        let failure_status = 1 << 8;
        #[cfg(windows)]
        let failure_status = 1;
        let output = |success: bool, stdout: &[u8]| Output {
            status: ExitStatus::from_raw(if success { 0 } else { failure_status }),
            stdout: stdout.to_vec(),
            stderr: Vec::new(),
        };
        let plausible_revision = b"123abcdef\n";
        assert_eq!(
            app_build_script::successful_git_stdout(output(true, plausible_revision)),
            Some("123abcdef".to_owned())
        );
        assert_eq!(
            app_build_script::successful_git_stdout(output(false, plausible_revision)),
            None
        );
        assert_eq!(
            app_build_script::successful_git_stdout(output(true, b" \n")),
            None
        );
        assert_eq!(
            app_build_script::successful_git_stdout(output(true, &[0xff])),
            None
        );
    }
}
