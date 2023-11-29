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
use std::{fs, path::PathBuf, process::Command};

use anyhow::{bail, Context, Result};
use chrono::DateTime;
use glob::glob;
use oci_spec::image::{ImageIndex, ImageManifestBuilder, MediaType, RootFsBuilder};
use rusqlite::Connection;
use tempfile::TempDir;

use super::Lockfile;
use crate::write;
use crate::{config::Config, oci};

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
        // Set up the OCI image, and unpack it so we can edit the rootfs
        oci::init_image_directory(image)?;

        let tmp_rpm_dir: TempDir; // This needs to stay in scope til end of fn, if set
        let rpm_dir = if let Some(vendor_dir) = vendor_dir {
            PathBuf::from(vendor_dir)
        } else {
            // If no vendor dir is provided then attempt to download the packages
            tmp_rpm_dir = TempDir::new()?;
            self.download_rpms(cfg, tmp_rpm_dir.path())?;
            PathBuf::from(tmp_rpm_dir.path())
        };

        // Verify signatures of packages
        self.check_gpg_keys(&rpm_dir)?;

        // Install the RPMs into a new directory that will become the container rootfs
        let tmp_dir = TempDir::new()?;
        let installroot = PathBuf::from(tmp_dir.path());
        let mut dnf_install = Command::new("dnf");

        let creation_time = if let Ok(sde) = std::env::var("SOURCE_DATE_EPOCH") {
            let timestamp = sde
                .parse::<i64>()
                .with_context(|| format!("Failed to parse SOURCE_DATE_EPOCH `{}`", sde))?;
            DateTime::from_timestamp(timestamp, 0)
                .ok_or_else(|| anyhow::anyhow!("SOURCE_DATE_EPOCH out of range: `{}`", sde))?
        } else {
            chrono::Utc::now()
        };

        dnf_install
            .env("SOURCE_DATE_EPOCH", creation_time.timestamp().to_string())
            .arg("--disablerepo=*")
            .arg("--installroot")
            .arg(&installroot)
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

        // Add any local packages.
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
        let _ = fs::remove_file(installroot.join("var/lib/rpm/.rpm.lock"));
        // rpm configures sqlite to persist the WAL and SHM files: https://github.com/rpm-software-management/rpm/blob/1cd9f9077a2829c363a198e5af56c8a56c6bc346/lib/backend/sqlite.c#L174C35-L174C59
        // this is a source of non-determinism, so we disable it here (should rpm need to be run against this db, it will re-create the journaling files)
        // This obviously only helps if RPM uses sqlite for the database and stores it in /var/lib/rpm
        let sqlite_shm = installroot.join("var/lib/rpm/rpmdb.sqlite-shm");
        if sqlite_shm.exists() {
            disable_sqlite_journaling(&installroot.join("var/lib/rpm/rpmdb.sqlite"))
                .context("Failed to disable sqlite journaling of RPM db")?;
        }

        // Create the root filesystem layer
        write::ok("Creating", "root filesystem layer")?;
        let (layer, diff_id) = match oci::create_image_layer(&installroot, image, creation_time) {
            Ok((layer, diff_id)) => (layer, diff_id),
            Err(e) => {
                let p = tmp_dir.into_path();
                write::error(
                    "Failed",
                    format!(
                        "to create root filesystem layer. Keeping temporary directory for debugging: {}",
                        p.display()
                    ),
                )?;
                return Err(e);
            }
        };

        // Create the image configuration blob
        write::ok("Writing", "image configuration blob")?;
        let mut image_config = cfg
            .image
            .to_oci_image_configuration(labels, creation_time)?;
        let rootfs = RootFsBuilder::default().diff_ids(vec![diff_id]).build()?;
        image_config.set_rootfs(rootfs);
        let config = oci::write_json_blob(&image_config, MediaType::ImageConfig, image)?;

        // Create the image manifest
        write::ok("Writing", "image manifest")?;
        let manifest = ImageManifestBuilder::default()
            .schema_version(2u32)
            .media_type(MediaType::ImageManifest)
            .layers(vec![layer])
            .config(config)
            .build()?;
        let mut manifest_descriptor =
            oci::write_json_blob(&manifest, MediaType::ImageManifest, image)?;
        let mut annotations = HashMap::new();
        annotations.insert(
            "org.opencontainers.image.ref.name".to_string(),
            tag.to_string(),
        );
        manifest_descriptor.set_annotations(Some(annotations));

        // Add the manifest descriptor to the OCI image index
        write::ok("Adding", "manifest to OCI image index")?;
        let index_path = Path::new(image).join("index.json");
        let mut index: ImageIndex = serde_json::from_str(
            &fs::read_to_string(&index_path)
                .context(format!("Failed to read `{}`", index_path.display()))?,
        )?;

        // Remove any image with the same name
        let mut manifests = index
            .manifests()
            .iter()
            .filter(|manifest| {
                let name = manifest
                    .annotations()
                    .as_ref()
                    .and_then(|map| map.get("org.opencontainers.image.ref.name"));
                if let Some(name) = name {
                    name != tag
                } else {
                    true
                }
            })
            .cloned()
            .collect::<Vec<_>>();
        manifests.push(manifest_descriptor);
        index.set_manifests(manifests);

        let index_file = std::fs::File::create(&index_path).context(format!(
            "Failed to create index.json file `{}`",
            index_path.display()
        ))?;
        serde_json::to_writer(index_file, &index).context(format!(
            "Failed to write to index.json file `{}`",
            index_path.display()
        ))?;
        Ok(())
    }
}

fn disable_sqlite_journaling(path: &Path) -> Result<()> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "DELETE")?;
    Ok(())
}
