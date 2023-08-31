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
use std::{os::unix::process::CommandExt, process::Command};

use anyhow::{bail, Context, Result};
use clap::Parser;
use nix::{
    sched::CloneFlags,
    sys::{
        signal::{self, Signal},
        wait::wait,
    },
    unistd::{close, getgid, getuid, pipe, read},
};
use rpmoci::{subid::setup_id_maps, write};

fn main() {
    if let Err(err) = try_main() {
        write::error("Error", err.to_string()).unwrap();
        err.chain()
            .skip(1)
            .for_each(|cause| eprintln!("caused by: {}", cause));
        std::process::exit(1);
    }
}

fn run_in_userns() -> anyhow::Result<()> {
    // dnf needs to be run as root, but given that rpmoci only needs to query package repos
    // and/or install packages into an install root, we can run in a user namespace, mapping
    // the current uid/gid to root
    // this function spawns a child process in a new user namespace, the parent configures
    // the uid/gid mappings, then signals the child to re-exec rpmoci

    let user_id = getuid();
    let group_id = getgid();
    // dnf chooses a root-writable cache directory by default. It's unlikely that
    // the current user will be able to write there, so we configure the RPMOCI_CACHE_DIR
    // to point to a cache directory in the current user's home directory.
    let cache_dir = dirs::cache_dir().unwrap().join("rpmoci");

    const STACK_SIZE: usize = 1024 * 1024;
    let stack: &mut [u8; STACK_SIZE] = &mut [0; STACK_SIZE];
    // this pipe is for the parent to notify the child when user namespaces mappings
    // have been configured
    let (reader, writer) = pipe()?;
    // create child process with a new user namespace
    let child = nix::sched::clone(
        Box::new(|| {
            // this child process just waits for the parent to notify it before re-execing
            close(writer).unwrap();
            read(reader, &mut Vec::new()).unwrap();
            Command::new(std::env::current_exe().unwrap())
                .args(std::env::args().skip(1))
                .env("RPMOCI_CACHE_DIR", cache_dir.clone())
                .exec();
            255
        }),
        stack,
        CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNS,
        None,
    )
    .context("Clone failed")?;

    // this parent process sets up user namespace mappings, notifies the child to continue,
    // then waits for the child to exit
    close(reader)?;
    // Kill the child process if we fail to setup the uid/gid mappings
    if let Err(e) =
        setup_id_maps(child, user_id, group_id).context("Failed to setup uid/gid mappings")
    {
        signal::kill(child, Signal::SIGTERM)?;
        return Err(e);
    }
    close(writer)?;
    let status = wait()?;
    if let nix::sys::wait::WaitStatus::Exited(_, code) = status {
        // Exit immediately with the child's exit code, as the child should have
        // have already printed any error messages on completion
        std::process::exit(code);
    } else {
        bail!("Child process failed");
    }
}

fn try_main() -> Result<()> {
    let args = rpmoci::cli::Cli::parse();
    env_logger::Builder::new()
        .filter_level(args.verbose.log_level_filter())
        .init();

    if !getuid().is_root() {
        run_in_userns().context("Failed to run rpmoci in rootless mode. See https://github.com/microsoft/rpmoci#rootless-setup, or re-run as root")?;
    }
    rpmoci::main(args.command)
}
