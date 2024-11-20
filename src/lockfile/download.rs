//! Copyright (C) Microsoft Corporation.
//!
//! This program is free software: you can redistribute it and/or modify
//! it under the terms of the GNU General Public License as published by
//! the Free Software Foundation, either version 3 of the License, or
//! (at your option) any later version.
//!
//! This program is distributed in the hope that it will be useful,
//! but WITHOUT ANY WARRANTY; without even the implied warranty of
//! MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
//! GNU General Public License for more details.
//!
//! You should have received a copy of the GNU General Public License
//! along with this program.  If not, see <https://www.gnu.org/licenses/>.
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::path::Path;
use std::{io::Write, process::Command};

use anyhow::{bail, Context, Result};
use pyo3::ffi::c_str;
use pyo3::prelude::*;
use pyo3::types::PyTuple;
use tempfile::{tempdir, TempDir};

use super::resolve::setup_base;
use super::Lockfile;
use crate::config::Config;
use crate::write;

impl Lockfile {
    /// Download RPMs to a given directory
    pub fn download_rpms(&self, cfg: &Config, dir: &Path) -> Result<()> {
        let repositories = &cfg.contents.repositories;

        Python::with_gil(|py| {
            let base = setup_base(py, repositories, &cfg.contents.gpgkeys)?;
            let download = PyModule::from_code(
                py,
                c_str!(include_str!("download.py")),
                c_str!("resolve"),
                c_str!("resolve"),
            )?;

            let packages = self
                .packages
                .iter()
                .map(|p| (p.name.clone(), p.evr.clone(), p.checksum.checksum.clone()))
                .collect::<Vec<_>>();

            let args = PyTuple::new(
                py,
                [
                    base.as_any(),
                    packages.into_pyobject(py)?.as_any(),
                    dir.into_pyobject(py)?.as_any(),
                ],
            )?;
            // Run the download function
            download.getattr("download")?.call1(args)?;
            Ok::<_, anyhow::Error>(())
        })
        .context("Failed to download dependencies with dnf")
    }

    /// Check GPG keys of downloaded packages against the GPG keys stored in the lockfile
    pub fn check_gpg_keys(&self, dir: &Path) -> Result<()> {
        // Overview:
        // 1. create temporary directory
        // 2. use rpm to import all keys from the lockfile into that directory
        // 3. use rpmkeys to verify each download package
        let tmp_dir = tempdir()?;

        write::ok("Verifying", "RPM signatures")?;
        // Load GPG keys into a new rpm db
        for (repoid, repo_key_info) in &self.repo_gpg_config {
            if repo_key_info.gpgcheck {
                for (i, key) in repo_key_info.keys.iter().enumerate() {
                    load_key(&tmp_dir, &format!("{}-{}", repoid, i), key)?;
                }
            }
        }

        // Get list of RPM names whose signatures need to be verified
        let gpgcheck_repoids = self
            .repo_gpg_config
            .iter()
            .filter_map(|(repoid, repo_key_info)| {
                if repo_key_info.gpgcheck {
                    Some(repoid)
                } else {
                    None
                }
            })
            .collect::<HashSet<_>>();
        let gpgcheck_pkg_names = self
            .packages
            .iter()
            .filter_map(|p| {
                if gpgcheck_repoids.contains(&p.repoid) {
                    Some(p.name.as_str())
                } else {
                    None
                }
            })
            .collect::<HashSet<_>>();

        // verify RPMs in the directory
        for file in fs::read_dir(dir)? {
            let path = file?.path();
            if path.extension() == Some(OsStr::new("rpm")) {
                let pkg = rpm::Package::open(&path).map_err(|e| {
                    anyhow::anyhow!(
                        "Failed to open RPM package {}: {}",
                        path.display(),
                        e.to_string()
                    )
                })?;
                if gpgcheck_pkg_names.contains(pkg.metadata.get_name().map_err(|e| {
                    anyhow::anyhow!(
                        "Failed to get RPM name {}: {}",
                        path.display(),
                        e.to_string()
                    )
                })?) {
                    check_pkg_signature(&path, tmp_dir.path())?;
                }
            }
        }

        Ok(())
    }
}

fn load_key(tmp_dir: &TempDir, name: &str, key: &str) -> Result<(), anyhow::Error> {
    let gpg_path = tmp_dir.path().join(name);
    let mut gpg_key =
        File::create(&gpg_path).context(format!("Failed to create {}", gpg_path.display()))?;
    gpg_key
        .write_all(key.as_bytes())
        .context("Failed to write gpg key")?;
    gpg_key.flush()?;
    Command::new("rpm")
        .arg("--root")
        .arg(tmp_dir.path())
        .arg("--import")
        .arg(gpg_path)
        .status()
        .context("Failed to run `rpm`")?;
    Ok(())
}

/// Verify a package signature using rpmkeys
fn check_pkg_signature(rpm_path: &Path, root: &Path) -> Result<()> {
    let output = Command::new("rpmkeys")
        .arg("--root")
        .arg(root)
        .arg("--checksig")
        .arg(rpm_path)
        .output()
        .context("Failed to run `rpmkeys`")?;

    if !output.status.success() {
        bail!(
            "rpmkeys failed: {}{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.contains("digests signatures OK") {
        bail!("rpm verification failed: {}", stdout);
    }

    Ok(())
}
