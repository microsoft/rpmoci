//! Module for operations involving a lockfile
//!
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
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::write;
use crate::{config::Config, NAME};

mod build;
mod download;
mod resolve;

/// Represents an rpmoci lockfile
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Lockfile {
    pkg_specs: Vec<String>,
    packages: BTreeSet<Package>,
    #[serde(default)]
    local_packages: BTreeSet<LocalPackage>,
    #[serde(default)]
    repo_gpg_config: HashMap<String, RepoKeyInfo>,
    #[serde(default)]
    global_key_specs: Vec<url::Url>,
}

/// A package that the user has specified locally
/// Note that we don't store the package version or path in the lockfile,
/// but instead re-do our search for local packages at install time.
///
/// This enables the version of local RPMs to change without breaking compatibility
/// with the lockfile. In particular, the local RPM's version can change without
/// re-resolving the lockfile.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, PartialOrd, Eq, Ord)]
pub struct LocalPackage {
    /// The path to the package
    name: String,
    /// The RPM requires
    requires: Vec<String>,
}

/// Format of dnf resolve script output
#[derive(Debug, Serialize, Deserialize)]
struct DnfOutput {
    /// The resolved remote packages
    packages: Vec<Package>,
    /// Local packages
    local_packages: Vec<LocalPackage>,
    /// Repository GPG configuration
    repo_gpg_config: HashMap<String, RepoKeyInfo>,
}

/// GPG key configuration for a specified repository
#[derive(Debug, Serialize, Deserialize, Clone)]
struct RepoKeyInfo {
    /// Is GPG checking enabled for this repository
    gpgcheck: bool,
    /// contents of any keys specified via repository configuration
    keys: Vec<String>,
}

/// A resolved package
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, PartialOrd, Eq, Ord)]
struct Package {
    /// The package name
    name: String,
    /// The package epoch-version-release
    evr: String,
    /// The package checksum
    checksum: Checksum,
    /// The id of the package's repository
    repoid: String,
}

/// Checksum of RPM package
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, PartialOrd, Eq, Ord)]
struct Checksum {
    /// The algorithm of the checksum
    algorithm: Algorithm,
    /// The checksum value
    checksum: String,
}

/// Algorithms supported by RPM for checksums
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, PartialOrd, Eq, Ord)]
#[serde(rename_all = "lowercase")]
enum Algorithm {
    MD5,  //Devskim: ignore DS126858
    SHA1, //Devskim: ignore DS126858
    SHA256,
    SHA384,
    SHA512,
}

impl Lockfile {
    /// Returns true if the lockfile is compatible with the
    /// given configuration, false otherwise
    #[must_use]
    pub fn is_compatible(&self, cfg: &Config) -> bool {
        self.pkg_specs == cfg.contents.packages && self.global_key_specs == cfg.contents.gpgkeys
    }

    /// Returns true if the lockfile is compatible with the specified configuration
    /// and if all rpm packages required by local dependencies are included in the lockfile.
    pub fn all_local_deps_compatible(&self, cfg: &Config) -> Result<bool> {
        let local_package_deps: BTreeSet<LocalPackage> = self.local_packages.clone();

        Ok(self.pkg_specs == cfg.contents.packages
            && self.global_key_specs == cfg.contents.gpgkeys
            // Verify dependencies of all local packages
            && Self::read_local_rpm_deps(cfg)?.iter().all(|x| {
                // Check that there is still a local package with the same name
                local_package_deps.iter().any(|y| x.name == y.name) &&
                // Check that the local package still has the same dependencies
                local_package_deps.clone().into_iter().all(|y| {
                    // Note that the orders or the requires vector matters here.
                    // The rpm query returns in a different order to the lockfile
                    // (Requires(pre), Requires, then Requires(post)) so sorting is needed.
                    let mut y_requires = y.requires.clone();
                    let mut x_requires = x.requires.clone();
                    y_requires.sort();
                    x_requires.sort();
                    if x.name == y.name {
                        y_requires == x_requires
                    }
                    else {
                        // If the local package names don't match, then we return true as
                        // the case where the name of the local package changing/not matching
                        // is handled in the above `any` check
                        true
                    }
                })
            }))
    }

    /// Write the lockfile to a file on disk
    pub fn write_to_file(&self, path: impl AsRef<Path>) -> Result<()> {
        let mut lock = std::fs::File::create(path.as_ref())?;
        lock.write_all(
            format!(
                "# This file is @generated by {}\n# It is not intended for manual editing.\n",
                NAME.to_ascii_uppercase(),
            )
            .as_bytes(),
        )?;
        lock.write_all(toml::to_string_pretty(&self)?.as_bytes())?;
        Ok(())
    }

    /// Print messages to stderr showing changes from a previous lockfile.
    pub fn print_updates(&self, previous: Option<&Lockfile>) -> Result<()> {
        let mut new = self
            .packages
            .iter()
            .map(|pkg| (&pkg.name, &pkg.evr))
            .collect::<BTreeMap<_, _>>();
        let old = previous
            .map(|previous| {
                previous
                    .packages
                    .iter()
                    .map(|pkg| (&pkg.name, &pkg.evr))
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();

        for (name, evr) in old {
            if let Some(new_evr) = new.remove(name) {
                if new_evr != evr {
                    write::ok("Updating", format!("{} {} -> {}", name, evr, new_evr))?;
                }
            } else {
                write::ok("Removing", format!("{} {}", name, evr))?;
            }
        }
        for (name, evr) in new {
            write::ok("Adding", format!("{} {}", name, evr))?;
        }

        Ok(())
    }
}
