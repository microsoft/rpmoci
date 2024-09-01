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
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::Path;
use std::{fs, process::Command};

use anyhow::{bail, Context, Result};
use chrono::DateTime;
use flate2::Compression;
use glob::glob;
use ocidir::oci_spec::image::MediaType;
use ocidir::{new_empty_manifest, OciDir};
use rusqlite::Connection;
use tempfile::TempDir;

use super::Lockfile;
use crate::archive::append_dir_all_with_xattrs;
use crate::config::Config;
use crate::write;
use ocidir::cap_std::fs::Dir;

const CREATED_BY: &str = "Created by rpmoci";

impl Lockfile {
    /// Build a container image from a lockfile
    pub fn build(
        &self,
        cfg: &Config,
        image: &str,
        tag: &str,
        vendor_dir: Option<&Path>,
        labels: HashMap<String, String>,
    ) -> Result<()> {
        // Ensure OCI directory exists
        fs::create_dir_all(image)
            .context(format!("Failed to create OCI image directory `{}`", &image))?;
        let dir = Dir::open_ambient_dir(image, ocidir::cap_std::ambient_authority())
            .context("Failed to open image directory")?;
        let oci_dir = OciDir::ensure(&dir)?;

        let creation_time = creation_time()?;
        let installroot = TempDir::new()?; // This needs to outlive the layer builder below.
        if let Some(vendor_dir) = vendor_dir {
            // Use vendored RPMs rather than downloading
            self.create_installroot(installroot.path(), vendor_dir, false, cfg, &creation_time)
        } else {
            // No vendoring - download RPMs
            let tmp_rpm_dir = TempDir::new()?;
            self.create_installroot(
                installroot.path(),
                tmp_rpm_dir.path(),
                true,
                cfg,
                &creation_time,
            )
        }
        .context("Failed to create installroot")?;

        // Create the root filesystem layer
        write::ok("Creating", "root filesystem layer")?;
        let mut builder = oci_dir.create_layer(Compression::fast().into())?;
        builder.follow_symlinks(false);
        append_dir_all_with_xattrs(&mut builder, installroot.path(), creation_time.timestamp())
            .context("failed to archive root filesystem")?;
        let layer = builder.into_inner()?.complete()?;

        // Create the image configuration blob
        write::ok("Writing", "image configuration blob")?;
        let mut image_config = cfg
            .image
            .to_oci_image_configuration(labels, creation_time)?;
        // Create the image manifest
        let mut manifest = new_empty_manifest()
            .media_type(MediaType::ImageManifest)
            .build()?;
        oci_dir.push_layer_full(
            &mut manifest,
            &mut image_config,
            layer,
            Option::<HashMap<String, String>>::None,
            CREATED_BY,
            creation_time,
        );

        write::ok("Writing", "image manifest and config")?;
        oci_dir.insert_manifest_and_config(
            manifest,
            image_config,
            Some(tag),
            ocidir::oci_spec::image::Platform::default(),
        )?;
        Ok(())
    }

    fn create_installroot(
        &self,
        installroot: &Path,
        rpm_dir: &Path,
        download_rpms: bool,
        cfg: &Config,
        creation_time: &DateTime<chrono::Utc>,
    ) -> Result<(), anyhow::Error> {
        if download_rpms {
            self.download_rpms(cfg, rpm_dir)?;
        }
        self.check_gpg_keys(rpm_dir)?;
        let mut dnf_install = Command::new("dnf");
        dnf_install
            .env("SOURCE_DATE_EPOCH", creation_time.timestamp().to_string())
            .arg("--disablerepo=*")
            .arg("--installroot")
            .arg(installroot)
            .arg("install")
            .arg("--assumeyes")
            .arg(format!(
                "--setopt=tsflags={}",
                if cfg.contents.docs { "" } else { "nodocs" }
            ))
            // Add remote RPMs from the download or vendor dir
            .args({
                let mut rpm_paths = Vec::new();
                for file in fs::read_dir(rpm_dir)? {
                    let path = file?.path();
                    if path.extension() == Some(OsStr::new("rpm")) {
                        rpm_paths.push(path);
                    }
                }
                rpm_paths
            });
        for glob_spec in cfg
            .contents
            .packages
            .iter()
            .filter(|spec| spec.ends_with(".rpm"))
        {
            let mut found = false;
            for entry in glob(glob_spec)? {
                dnf_install.arg(entry?);
                found = true;
            }
            if !found {
                bail!("No package found for spec '{}'", glob_spec);
            }
        }
        write::ok("Installing", "packages")?;
        log::debug!("Running `{:?}`", dnf_install);
        let status = dnf_install.status().context("Failed to run dnf")?;
        if !status.success() {
            bail!("failed to dnf install");
        }
        write::ok("Installed", "packages successfully")?;

        // Remove unnecessary installation artifacts from the rootfs if present
        let _ = fs::remove_dir_all(installroot.join("var/log"));
        let _ = fs::remove_dir_all(installroot.join("var/cache"));
        let _ = fs::remove_dir_all(installroot.join("var/tmp"));
        let _ = fs::remove_dir_all(installroot.join("var/lib/dnf/"));

        // rpm configures sqlite to persist the WAL and SHM files: https://github.com/rpm-software-management/rpm/blob/1cd9f9077a2829c363a198e5af56c8a56c6bc346/lib/backend/sqlite.c#L174C35-L174C59
        // this is a source of non-determinism, so we disable it here (should rpm need to be run against this db, it will re-create the journaling files)
        let legacy_rpmdb = installroot.join("var/lib/rpm/rpmdb.sqlite");
        let rpmdb = installroot.join("usr/lib/sysimage/rpm/rpmdb.sqlite");
        if rpmdb.exists() {
            disable_sqlite_journaling(&rpmdb)
        } else if legacy_rpmdb.exists() {
            disable_sqlite_journaling(&legacy_rpmdb)
        } else {
            Ok(())
        }
        .context("Failed to disable sqlite journaling of RPM db")?;
        Ok(())
    }
}

fn creation_time() -> Result<DateTime<chrono::Utc>, anyhow::Error> {
    let creation_time = if let Ok(sde) = std::env::var("SOURCE_DATE_EPOCH") {
        let timestamp = sde
            .parse::<i64>()
            .with_context(|| format!("Failed to parse SOURCE_DATE_EPOCH `{}`", sde))?;
        DateTime::from_timestamp(timestamp, 0)
            .ok_or_else(|| anyhow::anyhow!("SOURCE_DATE_EPOCH out of range: `{}`", sde))?
    } else {
        chrono::Utc::now()
    };
    Ok(creation_time)
}

fn disable_sqlite_journaling(path: &Path) -> Result<()> {
    let shm_path = path.with_extension("sqlite-shm");
    if shm_path.exists() {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "DELETE")?;
    }

    Ok(())
}
