//! rpmoci CLI
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
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use clap_verbosity_flag::Verbosity;

/// Main CLI struct
#[derive(Debug, Parser)]
#[clap(
    about = "RPM-based OCI image builder",
    long_about = "See 'rpmoci help <subcommand>' for more information on a specific subcommand",
    version
)]
pub struct Cli {
    #[clap(subcommand)]
    /// The available subcommand
    pub command: Command,
    /// Verbosity
    #[clap(flatten)]
    pub verbose: Verbosity,
}

fn label_parser(s: &str) -> Result<(String, String), String> {
    match s.split_once('=') {
        Some((key, value)) => Ok((key.to_string(), value.to_string())),
        None => Err(format!("`{}` should be of the form KEY=VALUE.", s)),
    }
}

/// Subcommands
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Update dependencies as recorded in the local lock file
    Update {
        /// Path to rpmoci manifest file.
        /// By default, rpmoci searches for rpmoci.toml in the current directory.
        #[clap(short = 'f', long = "file", default_value = "rpmoci.toml")]
        manifest_path: PathBuf,
        /// Use the existing lock file to update dependencies.
        /// This allows the RPM dependencies in the lock file to be updated, without
        /// local RPMs being present, which may be useful in dependency updating scenarios.
        #[clap(long = "from-lockfile")]
        from_lockfile: bool,
    },
    /// Build an OCI image
    Build {
        /// Require that the lock file is up-to-date. rpmoci will exit with an
        /// an error if the lock file is missing or needs to be updated
        #[clap(long = "locked")]
        locked: bool,
        #[clap(long = "image")]
        /// Path to OCI image layout
        image: String,
        #[clap(long = "label", value_parser = label_parser)]
        /// Specify additional labels to apply to the image
        /// Labels are specified as KEY=VALUE
        label: Vec<(String, String)>,
        #[clap(long = "tag")]
        /// The tag to give the image in the specified OCI image layout
        tag: String,
        /// Optionally, use RPMs from a specified directory instead of downloading them.
        /// Example workflow: `rpmoci vendor --out-dir ./vendor` followed by
        /// `rpmoci build --image foo --tag bar --vendor-dir vendor`
        #[clap(long = "vendor-dir")]
        vendor_dir: Option<PathBuf>,
        /// Path to rpmoci manifest file.
        /// By default, rpmoci searches for rpmoci.toml in the current directory
        #[clap(short = 'f', long = "file", default_value = "rpmoci.toml")]
        manifest_path: PathBuf,
    },
    /// Vendor RPM dependencies locally
    Vendor {
        /// The directory in which to store downloaded RPMs.
        /// This can subsequently be passed to `rpmoci build`
        #[clap(long = "out-dir")]
        out_dir: PathBuf,
        /// Path to rpmoci manifest file.
        /// By default, rpmoci searches for rpmoci.toml in the current directory.
        #[clap(short = 'f', long = "file", default_value = "rpmoci.toml")]
        manifest_path: PathBuf,
    },
}
