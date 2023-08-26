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
use std::{fs::File, io::Write, os::unix::process::CommandExt, path::PathBuf, process::Command};

use anyhow::{bail, Context, Result};
use clap::Parser;
use nix::{
    sched::CloneFlags,
    sys::{signal, wait::wait},
    unistd::{close, getgid, getuid, pipe, read},
};
use rpmoci::write;

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
        CloneFlags::CLONE_NEWUSER,
        Some(signal::SIGCHLD as i32),
    )
    .context("Clone failed")?;

    // this parent process sets up user namespace mappings, notifies the child to continue,
    // then waits for the child to exit
    close(reader)?;
    let child_proc = PathBuf::from("/proc").join(child.to_string());
    File::create(child_proc.join("setgroups"))
        .context("failed to create setgroups file")?
        .write_all(b"deny")
        .context("failed to write to setgroups file")?;
    // map the current uid/gid to root, and create mappings for uids/gids 1-999 as
    // RPMs could potentially contain files owned by any of these
    // (these additional mappings are the cause of us needing to spawn a child - otherwise
    // we could just unshare and configure mappings in the current process)
    File::create(child_proc.join("uid_map"))?
        .write_all(format!("0 {user_id} 1\n1 100000 999").as_bytes())?;
    File::create(child_proc.join("gid_map"))?
        .write_all(format!("0 {group_id} 1\n1 100000 999").as_bytes())?;
    close(writer)?;
    let status = wait()?;
    if let nix::sys::wait::WaitStatus::Exited(_, code) = status {
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
        run_in_userns().context("Failed to run rpmoci in user namespace")?;
    }
    rpmoci::main(args.command)
}
