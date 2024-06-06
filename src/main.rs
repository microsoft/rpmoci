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
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::Command,
};

use anyhow::{bail, Context, Result};
use clap::Parser;
use nix::{
    libc::c_int,
    sched::CloneFlags,
    sys::{
        signal::{
            self,
            Signal::{self, SIGCHLD},
        },
        wait::waitpid,
    },
    unistd::{close, getgid, getuid, pipe, read},
};
use pyo3::{
    types::{PyAnyMethods, PyModule},
    Python,
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

unsafe fn run_in_userns() -> anyhow::Result<()> {
    // dnf needs to be run as root, but given that rpmoci only needs to query package repos
    // and/or install packages into an install root, we can run in a user namespace, mapping
    // the current uid/gid to root
    // this function spawns a child process in a new user namespace, the parent configures
    // the uid/gid mappings, then signals the child to re-exec rpmoci

    let user_id = getuid();
    let group_id = getgid();
    let cache_dir =
        get_cache_dir().context("Failed to determine a user-writable cache dir for dnf")?;

    const STACK_SIZE: usize = 1024 * 1024;
    let stack: &mut [u8; STACK_SIZE] = &mut [0; STACK_SIZE];
    // this pipe is for the parent to notify the child when user namespaces mappings
    // have been configured
    let (reader, writer) = pipe()?;
    // create child process with a new user namespace
    let child = nix::sched::clone(
        Box::new(|| {
            // this child process just waits for the parent to notify it before re-execing
            close(writer.as_raw_fd()).unwrap();

            read(
                reader.as_raw_fd(),
                // Pass a non-zero length buffer to read() to ensure the child blocks
                &mut [0u8; 1],
            )
            .unwrap();
            Command::new(std::env::current_exe().unwrap())
                .args(std::env::args().skip(1))
                .env("RPMOCI_CACHE_DIR", cache_dir.clone())
                .exec();
            255
        }),
        stack,
        CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNS,
        Some(SIGCHLD as c_int),
    )
    .context("Clone failed")?;

    // this parent process sets up user namespace mappings, notifies the child to continue,
    // then waits for the child to exit
    close(reader.as_raw_fd())?;
    // Kill the child process if we fail to setup the uid/gid mappings
    if let Err(e) =
        setup_id_maps(child, user_id, group_id).context("Failed to setup uid/gid mappings")
    {
        signal::kill(child, Signal::SIGTERM)?;
        waitpid(child, None)?;
        return Err(e);
    }
    close(writer.as_raw_fd())?;
    let status = waitpid(child, None)?;
    if let nix::sys::wait::WaitStatus::Exited(_, code) = status {
        // Exit immediately with the child's exit code, as the child should have
        // have already printed any error messages on completion
        std::process::exit(code);
    } else {
        bail!("Child process failed");
    }
}

fn get_cache_dir() -> Result<String> {
    Python::with_gil(|py| {
        let misc = PyModule::import_bound(py, "dnf.yum.misc")?;
        misc.call_method0("getCacheDir")?;
        Ok(misc.getattr("getCacheDir")?.call0()?.extract()?)
    })
}

fn try_main() -> Result<()> {
    let args = rpmoci::cli::Cli::parse();
    env_logger::Builder::new()
        .filter_level(args.verbose.log_level_filter())
        .init();

    // `rpmoci build` is the only command that needs to run as root.
    // If a user specifies this command when running as a non-root user, then try and run
    // in rootless mode using a user namespace
    if matches!(args.command, rpmoci::cli::Command::Build { .. }) && !getuid().is_root() {
        unsafe {
            run_in_userns().context("Failed to run rpmoci in rootless mode. See https://github.com/microsoft/rpmoci#rootless-setup, or re-run as root")?;
        }
    }
    rpmoci::main(args.command)
}
