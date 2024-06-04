#![deny(missing_docs)]
//! Create container images using DNF
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
use std::{
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

use anyhow::{bail, Context};
pub mod cli;
pub mod config;
pub mod lockfile;
mod oci;
mod sha256_writer;
pub mod write;
use anyhow::Result;
use cli::Command;
use config::Config;
use lockfile::Lockfile;

pub(crate) const NAME: &str = "rpmoci";

fn load_config_and_lock_file(
    config_file: impl AsRef<Path>,
) -> Result<(Config, PathBuf, Result<Option<Lockfile>>)> {
    let config_file = config_file.as_ref();
    let contents = std::fs::read_to_string(config_file)
        .context(format!("Failed to read `{}`", config_file.display()))?;
    let cfg: Config = toml::from_str(&contents)?;
    let mut lockfile_path = PathBuf::from(config_file);
    lockfile_path.set_extension("lock");
    Ok((cfg, lockfile_path.clone(), read_lockfile(&lockfile_path)))
}

fn read_lockfile(lockfile: impl AsRef<Path>) -> Result<Option<Lockfile>> {
    match std::fs::read_to_string(lockfile) {
        Ok(d) => Ok(Some(toml::from_str(&d).context("Invalid lockfile")?)),
        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Run rpmoci
pub fn main(command: Command) -> anyhow::Result<()> {
    match command {
        Command::Update {
            manifest_path,
            from_lockfile,
        } => {
            let (cfg, lockfile_path, existing_lockfile) = load_config_and_lock_file(manifest_path)?;

            let lockfile = if let Ok(Some(lockfile)) = &existing_lockfile {
                if lockfile.is_compatible_excluding_local_rpms(&cfg) && from_lockfile {
                    lockfile.resolve_from_previous(&cfg)?
                } else {
                    if from_lockfile {
                        bail!("the lock file is not up-to-date. Use of --from-lockfile requires that the lock file is up-to-date");
                    }
                    Lockfile::resolve_from_config(&cfg)?
                }
            } else {
                if from_lockfile {
                    bail!("the lock file is not up-to-date. Use of --from-lockfile requires that the lock file is up-to-date");
                }
                Lockfile::resolve_from_config(&cfg)?
            };

            lockfile.print_updates(existing_lockfile.unwrap_or_default().as_ref())?;
            lockfile.write_to_file(lockfile_path)?;
        }
        Command::Build {
            locked,
            image,
            tag,
            vendor_dir,
            manifest_path,
            label,
        } => {
            let now = Instant::now();
            let mut changed = false;
            let (cfg, lockfile_path, existing_lockfile) = load_config_and_lock_file(manifest_path)?;
            let lockfile = match (existing_lockfile, locked) {
                (Ok(Some(lockfile)), true) => {
                    // TODO: consider whether this can move to including local RPMs. (Subtlety here is that may
                    // break scenarios where the user is using local RPMs that have a subset of the locked local RPM dependencies.)
                    if !lockfile.is_compatible_excluding_local_rpms(&cfg) {
                        bail!(format!(
                            "the lock file {} needs to be updated but --locked was passed to prevent this",
                            lockfile_path.display()
                        ));
                    }
                    lockfile
                }
                (Ok(Some(lockfile)), false) => {
                    if lockfile.is_compatible_including_local_rpms(&cfg)? {
                        // Compatible lockfile, use it
                        lockfile
                    } else {
                        // Incompatible lockfile, update it
                        changed = true;
                        write::ok(
                            "Generating",
                            format!(
                                "new lock file. The existing lock file {} is not up-to-date.",
                                lockfile_path.display()
                            ),
                        )?;
                        Lockfile::resolve_from_config(&cfg)?
                    }
                }
                (Err(err), false) => {
                    write::error(
                        "Warning",
                        format!(
                            "failed to parse existing lock file. Generating a new one. Error: {}",
                            err
                        ),
                    )?;
                    err.chain()
                        .skip(1)
                        .for_each(|cause| eprintln!("caused by: {}", cause));
                    changed = true;
                    Lockfile::resolve_from_config(&cfg)?
                }
                (Err(err), true) => {
                    return Err(err.context(format!(
                        "failed to parse existing lock file {}",
                        lockfile_path.display()
                    )))
                }
                (Ok(None), true) => {
                    bail!(format!(
                    "the lock file {} is missing and needs to be generated but --locked was passed to prevent this",
                    lockfile_path.display()
                ))
                }
                (Ok(None), false) => {
                    changed = true;
                    Lockfile::resolve_from_config(&cfg)?
                }
            };

            if changed {
                lockfile.write_to_file(lockfile_path)?;
            }

            lockfile.build(
                &cfg,
                &image,
                &tag,
                vendor_dir.as_deref(),
                label.into_iter().collect(),
            )?;
            let elapsed_time = now.elapsed();
            write::ok(
                "Success",
                format!(
                    "image '{}:{}' created in {:2}s",
                    image,
                    tag,
                    elapsed_time.as_secs_f32()
                ),
            )?;
        }
        Command::Vendor {
            out_dir,
            manifest_path,
        } => {
            fs::create_dir_all(&out_dir).context("Failed to create vendor directory")?;
            let (cfg, _lockfile_path, existing_lockfile) =
                load_config_and_lock_file(manifest_path)?;

            if let Ok(Some(lockfile)) = existing_lockfile {
                if lockfile.is_compatible_excluding_local_rpms(&cfg) {
                    lockfile.download_rpms(&cfg, &out_dir)?;
                    lockfile.check_gpg_keys(&out_dir)?;
                } else {
                    bail!(
                        "Lockfile out of date. `vendor` can only be run with a compatible lockfile"
                    )
                }
            } else {
                bail!(
                    "No valid lockfile found. `vendor` can only be run with a compatible lockfile"
                )
            }
        }
    }
    Ok(())
}
