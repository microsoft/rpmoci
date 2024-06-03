//! Integration Tests for rpmoci

use std::{
    fs::{self},
    path::PathBuf,
    process::Command,
};

use rpmoci::lockfile::Lockfile;

// Path to rpmoci binary under test
const EXE: &str = env!("CARGO_BIN_EXE_rpmoci");

fn setup_test(fixture: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/")
        .join(fixture);
    let out = root.join("out");
    let _ = fs::remove_dir_all(&out);
    fs::create_dir_all(&out).unwrap();
    fs::copy(root.join("rpmoci.toml"), out.join("rpmoci.toml")).unwrap();

    let lock = root.join("rpmoci.lock");
    if lock.exists() {
        fs::copy(lock, out.join("rpmoci.lock")).unwrap();
    }
    out
}

#[test]
fn test_incompatible_lockfile() {
    // Building with locked should fail
    let root = setup_test("incompatible_lockfile");
    let output = Command::new(EXE)
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
    let output = Command::new(EXE)
        .arg("update")
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(output.status.success());
}

#[test]
fn test_updatable_lockfile() {
    let root = setup_test("updatable_lockfile");
    let output = Command::new(EXE)
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
    let root = setup_test("unparseable_lockfile");
    // building with --locked should fail
    let output = Command::new(EXE)
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
    let output = Command::new(EXE)
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
    let root = setup_test("no_lockfile");
    // building with --locked should fail
    let output = Command::new(EXE)
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
    let root = setup_test("update_from_lockfile");
    let output = Command::new(EXE)
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

// This test requires oras be installed, to check that the produced images are
// compatible with another OCI tool.
#[test]
fn test_simple_build() {
    let root = setup_test("simple_build");
    let output = Command::new("sudo")
        .arg(EXE)
        .arg("build")
        .arg("--image=foo")
        .arg("--tag=bar")
        .current_dir(&root)
        .env("NO_COLOR", "YES") // So the stderr checks below work
        .output()
        .unwrap();

    let oras_status = Command::new("sudo")
        .arg("oras")
        .arg("copy")
        .arg("--from-oci-layout")
        .arg("--to-oci-layout")
        .arg("foo:bar")
        .arg("baz:bar")
        .current_dir(&root)
        .status()
        .unwrap();

    // Open the lockfile and verify /etc/os-release was included as a dependency
    let lockfile_path = root.join("rpmoci.lock");
    eprintln!("lockfile_path: {}", lockfile_path.display());
    let lockfile: Lockfile = toml::from_str(&fs::read_to_string(lockfile_path).unwrap()).unwrap();
    assert!(lockfile
        .iter_packages()
        .any(|p| p.name == "mariner-release"));

    // Cleanup using sudo
    let _ = Command::new("sudo")
        .arg("rm")
        .arg("-rf")
        .arg(&root)
        .status()
        .unwrap();

    let stderr = std::str::from_utf8(&output.stderr).unwrap();
    eprintln!("stderr: {}", stderr);
    assert!(output.status.success());
    assert!(oras_status.success());
}

#[test]
fn test_simple_vendor() {
    let root = setup_test("simple_vendor");
    let output = Command::new(EXE)
        .arg("update")
        .current_dir(&root)
        .env("NO_COLOR", "YES") // So the stderr checks below work
        .output()
        .unwrap();
    let stderr = std::str::from_utf8(&output.stderr).unwrap();
    eprintln!("stderr: {}. {}. {}", stderr, root.display(), EXE);
    assert!(output.status.success());

    let output = Command::new(EXE)
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

#[cfg(feature = "test-docker")]
#[test]
fn test_capabilities() {
    let root = setup_test("capabilities");
    let status = Command::new("sudo")
        .arg(EXE)
        .arg("build")
        .arg("--image=capabilities")
        .arg("--tag=test")
        .current_dir(&root)
        .status()
        .unwrap();
    assert!(status.success());

    let status = Command::new("sudo")
        .arg("skopeo")
        .arg("copy")
        .arg("oci:capabilities:test")
        .arg("docker-daemon:capabilities:test")
        .current_dir(&root)
        .status()
        .unwrap();
    assert!(status.success());

    let output = Command::new("docker")
        .arg("run")
        .arg("capabilities:test")
        .current_dir(&root)
        .output()
        .unwrap();
    let stderr = std::str::from_utf8(&output.stderr).unwrap();
    eprintln!("stderr: {}", stderr);
    assert!(std::str::from_utf8(&output.stdout)
        .unwrap()
        .contains("cap_net_admin=ep"));
    assert!(status.success());
}

#[test]
fn test_no_auto_etc_os_release() {
    // Test that `contents.os_release = false` works
    let root = setup_test("no_auto_etc_os_release");
    let output = Command::new(EXE)
        .arg("update")
        .current_dir(&root)
        .output()
        .unwrap();
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
    let root = setup_test("etc_os_release_explicit");
    let output = Command::new(EXE)
        .arg("update")
        .current_dir(&root)
        .output()
        .unwrap();
    let stderr = std::str::from_utf8(&output.stderr).unwrap();
    eprintln!("stderr: {}. {}. {}", stderr, root.display(), EXE);
    assert!(output.status.success());
}
