//! Integration Tests for rpmoci
use std::{
    fs::{self},
    path::{Path, PathBuf},
    process::Command,
};

use rpmoci::lockfile::Lockfile;

use ocidir::oci_spec::image::ImageIndex;
use test_temp_dir::TestTempDir;
use testcontainers::runners::SyncRunner;
use testcontainers_modules::cncf_distribution::CncfDistribution;

// Path to rpmoci binary under test
const EXE: &str = env!("CARGO_BIN_EXE_rpmoci");

fn rpmoci() -> Command {
    // if running as root, don't unshare
    let is_root = unsafe { libc::geteuid() == 0 };
    if is_root {
        Command::new(EXE)
    } else {
        // Run in user namespace
        let mut cmd = Command::new("unshare");
        // Don't use --map-auto here as that doesn't work on Azure Linux 2.0's unshare
        // This will cause failures if tests install RPMs which create users
        cmd.arg("--map-root-user").arg("--user").arg(EXE);
        cmd
    }
}

fn setup_test(fixture: &str) -> (TestTempDir, PathBuf) {
    // the test_temp_dir macro can't handle the integration test module path not containing ::,
    // so construct our own item path
    let out = test_temp_dir::TestTempDir::from_complete_item_path(&format!(
        "it::{}",
        std::thread::current().name().unwrap()
    ));
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/")
        .join(fixture);
    fs::copy(
        root.join("rpmoci.toml"),
        out.as_path_untracked().join("rpmoci.toml"),
    )
    .unwrap();

    let lock = root.join("rpmoci.lock");
    if lock.exists() {
        fs::copy(lock, out.as_path_untracked().join("rpmoci.lock")).unwrap();
    }
    let path = out.as_path_untracked().to_path_buf();
    (out, path)
}

#[test]
fn test_incompatible_lockfile() {
    // Building with locked should fail
    let (_tmp_dir, root) = setup_test("incompatible_lockfile");
    let output = rpmoci()
        .arg("build")
        .arg("--locked")
        .args(["--image=foo", "--tag=bar"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = std::str::from_utf8(&output.stderr).unwrap();
    eprintln!("stderr: {}", stderr);
    assert!(stderr.contains("needs to be updated but --locked was passed to prevent this"));

    // Updating should succeed
    let output = rpmoci().arg("update").current_dir(&root).output().unwrap();
    assert!(output.status.success());
}

#[test]
fn test_updatable_lockfile() {
    let (_tmp_dir, root) = setup_test("updatable_lockfile");
    let output = rpmoci()
        .arg("update")
        .current_dir(root)
        .env("NO_COLOR", "YES") // So the stderr checks below work
        .output()
        .unwrap();
    assert!(output.status.success());
    let stderr = std::str::from_utf8(&output.stderr).unwrap();
    eprintln!("stderr: {}", stderr);
    assert!(stderr.contains("Updating etcd 3.5.1-1.cm2 -> "));
    assert!(stderr.contains("Updating filesystem 1.1-9.cm2 ->"));
    assert!(stderr.contains("Updating glibc 2.35-1.cm2 -> "));
    assert!(!stderr.contains("Removing"));
}

#[test]
fn test_unparseable_lockfile() {
    let (_tmp_dir, root) = setup_test("unparseable_lockfile");
    // building with --locked should fail
    let output = rpmoci()
        .arg("build")
        .arg("--locked")
        .args(["--image=foo", "--tag=bar"])
        .current_dir(&root)
        .output()
        .unwrap();
    let stderr = std::str::from_utf8(&output.stderr).unwrap();
    assert!(!output.status.success());
    eprintln!("stderr: {}", stderr);
    assert!(stderr.contains("failed to parse existing lock file"));

    // but we should be able to update it
    let output = rpmoci()
        .arg("update")
        .current_dir(root)
        .env("NO_COLOR", "YES") // So the stderr checks below work
        .output()
        .unwrap();
    let stderr = std::str::from_utf8(&output.stderr).unwrap();
    eprintln!("stderr: {}", stderr);
    assert!(output.status.success());
    assert!(stderr.contains("Adding tini-static "));
}

#[test]
fn test_no_lockfile() {
    let (_tmp_dir, root) = setup_test("no_lockfile");
    // building with --locked should fail
    let output = rpmoci()
        .arg("build")
        .arg("--locked")
        .args(["--image=foo", "--tag=bar"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = std::str::from_utf8(&output.stderr).unwrap();
    eprintln!("stderr: {}", stderr);
    assert!(
        stderr.contains("is missing and needs to be generated but --locked was passed to prevent ")
    );
}

#[test]
fn test_update_from_lockfile() {
    let (_tmp_dir, root) = setup_test("update_from_lockfile");
    let output = rpmoci()
        .arg("update")
        .arg("--from-lockfile")
        .current_dir(root)
        .env("NO_COLOR", "YES") // So the stderr checks below work
        .output()
        .unwrap();
    let stderr = std::str::from_utf8(&output.stderr).unwrap();
    eprintln!("stderr: {}", stderr);
    assert!(output.status.success());
    assert!(stderr.contains("Updating dnf 4.8.0-1.cm2 -> "));
}

// Do a simple container image build, verifying the reproducibility and /etc/os-release dependency.
#[test]
fn test_simple_build() {
    // Repeat the same build twice using same SOURCE_DATE_EPOCH and ensure the resulting images are identical
    let (_tmp_dir, root) = setup_test("simple_build");
    let source_date_epoch = "1701168547";
    let output1 = rpmoci()
        .arg("build")
        .arg("--image=foo")
        .arg("--tag=bar")
        .current_dir(&root)
        .env("NO_COLOR", "YES") // So the stderr checks below work
        .env("SOURCE_DATE_EPOCH", source_date_epoch)
        .output()
        .unwrap();
    let stderr = std::str::from_utf8(&output1.stderr).unwrap();
    eprintln!("stderr: {}", stderr);
    assert!(output1.status.success());

    // Open the lockfile and verify /etc/os-release was included as a dependency
    let lockfile_path = root.join("rpmoci.lock");
    eprintln!("lockfile_path: {}", lockfile_path.display());
    let lockfile: Lockfile = toml::from_str(&fs::read_to_string(lockfile_path).unwrap()).unwrap();
    assert!(lockfile
        .iter_packages()
        .any(|p| p.name == "mariner-release"));

    let stderr = std::str::from_utf8(&output1.stderr).unwrap();
    eprintln!("stderr: {}", stderr);
    assert!(output1.status.success());

    // Repeat the build, to ensure reproducing the same image works
    std::thread::sleep(std::time::Duration::from_secs(1));
    let output2 = rpmoci()
        .arg("build")
        .arg("--image=foo")
        .arg("--tag=bar2")
        .current_dir(&root)
        .env("NO_COLOR", "YES")
        .env("SOURCE_DATE_EPOCH", source_date_epoch)
        .output()
        .unwrap();
    let stderr = std::str::from_utf8(&output2.stderr).unwrap();
    eprintln!("stderr: {}", stderr);
    assert!(output2.status.success());

    let index = ImageIndex::from_file(root.join("foo").join("index.json")).unwrap();
    assert_eq!(index.manifests()[0].digest(), index.manifests()[1].digest());
}

#[test]
fn test_simple_vendor() {
    let (_tmp_dir, root) = setup_test("simple_vendor");
    let output = rpmoci()
        .arg("update")
        .current_dir(&root)
        .env("NO_COLOR", "YES") // So the stderr checks below work
        .output()
        .unwrap();
    let stderr = std::str::from_utf8(&output.stderr).unwrap();
    eprintln!("stderr: {}. {}. {}", stderr, root.display(), EXE);
    assert!(output.status.success());

    let output = rpmoci()
        .arg("vendor")
        .arg("--out-dir=.")
        .current_dir(&root)
        .env("NO_COLOR", "YES") // So the stderr checks below work
        .output()
        .unwrap();
    let stderr = std::str::from_utf8(&output.stderr).unwrap();
    eprintln!("stderr: {}", stderr);
    assert!(output.status.success());
}

#[test]
fn test_no_auto_etc_os_release() {
    // Test that `contents.os_release = false` works
    let (_tmp_dir, root) = setup_test("no_auto_etc_os_release");
    let output = rpmoci().arg("update").current_dir(&root).output().unwrap();
    let stderr = std::str::from_utf8(&output.stderr).unwrap();
    eprintln!("stderr: {}. {}. {}", stderr, root.display(), EXE);
    assert!(output.status.success());
    // Open the lockfile and verify /etc/os-release was not added as a dependency
    let lockfile_path = root.join("rpmoci.lock");
    eprintln!("lockfile_path: {}", lockfile_path.display());
    let lockfile: Lockfile = toml::from_str(&fs::read_to_string(lockfile_path).unwrap()).unwrap();
    assert!(!lockfile
        .iter_packages()
        .any(|p| p.name == "mariner-release"));
}

#[test]
fn test_explicit_etc_os_release() {
    // Test that resolution works when /etc/os-release explicitly added
    let (_tmp_dir, root) = setup_test("etc_os_release_explicit");
    let output = rpmoci().arg("update").current_dir(&root).output().unwrap();
    let stderr = std::str::from_utf8(&output.stderr).unwrap();
    eprintln!("stderr: {}. {}. {}", stderr, root.display(), EXE);
    assert!(output.status.success());
    // Open the lockfile and verify /etc/os-release was added as a dependency
    let lockfile_path = root.join("rpmoci.lock");
    eprintln!("lockfile_path: {}", lockfile_path.display());
    let lockfile: Lockfile = toml::from_str(&fs::read_to_string(lockfile_path).unwrap()).unwrap();
    assert_eq!(
        lockfile
            .iter_packages()
            .filter(|p| p.name == "mariner-release")
            .count(),
        1
    );
}

#[test]
fn test_weak_deps() {
    // Verify a build without weak dependencies succeeds
    let (_tmp_dir, root) = setup_test("weakdeps");
    let status = rpmoci()
        .arg("build")
        .arg("--image=weak")
        .arg("--tag=deps")
        .current_dir(&root)
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn test_base_arch() {
    // Verify a build using a repo with a $basearch variable in the URL succeeds
    let (_tmp_dir, root) = setup_test("basearch");
    let status = rpmoci()
        .arg("build")
        .arg("--image=base")
        .arg("--tag=arch")
        .current_dir(&root)
        .status()
        .unwrap();
    assert!(status.success());
}

#[cfg(feature = "test-docker")]
#[test]
fn test_capabilities() {
    let output = build_and_run("capabilities");
    let stderr = std::str::from_utf8(&output.stderr).unwrap();
    eprintln!("stderr: {}", stderr);
    assert!(std::str::from_utf8(&output.stdout)
        .unwrap()
        .contains("cap_net_admin=ep"));
    assert!(output.status.success());
}

#[cfg(feature = "test-docker")]
#[test]
fn test_hardlinks() {
    // This test checks that /usr/bin/ld has a hardlink, i.e that rpmoci hasn't copied the file
    let output = build_and_run("hardlinks");
    let stderr = std::str::from_utf8(&output.stderr).unwrap();
    eprintln!("stderr: {}", stderr);
    assert_eq!(std::str::from_utf8(&output.stdout).unwrap().trim(), "2");

    // Test we can push the image to a registry
    let distribution_node = CncfDistribution::default().start().unwrap();
    let push_image = format!(
        "localhost:{}/hardlinks:test",
        distribution_node.get_host_port_ipv4(5000).unwrap(),
    );
    let status = Command::new("docker")
        .arg("tag")
        .arg("hardlinks:test")
        .arg(&push_image)
        .status()
        .expect("failed to run container");
    assert!(status.success());
    let status = Command::new("docker")
        .arg("push")
        .arg(&push_image)
        .status()
        .expect("failed to push image to registry");
    assert!(status.success());
}

fn build_and_run(image: &str) -> std::process::Output {
    let (_tmp_dir, root) = setup_test(image);
    let status = rpmoci()
        .arg("build")
        .arg("--image")
        .arg(image)
        .arg("--tag=test")
        .current_dir(&root)
        .status()
        .expect("failed to run rpmoci");
    assert!(status.success());
    copy_to_docker(image, &root);
    let output = Command::new("docker")
        .arg("run")
        .arg(format!("{}:test", image))
        .output()
        .expect("failed to run container");
    assert!(output.status.success());
    output
}

fn copy_to_docker(image: &str, root: impl AsRef<Path>) {
    let status = Command::new("skopeo")
        .arg("copy")
        .arg(format!("oci:{}:test", image))
        .arg(format!("docker-daemon:{}:test", image))
        .current_dir(root.as_ref())
        .status()
        .expect("failed to run skopeo");
    assert!(status.success());
}
