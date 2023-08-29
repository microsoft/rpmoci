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
use std::{fs::read_to_string, os::unix::process::CommandExt, path::Path, process::Command};

use anyhow::{bail, Context, Result};
use clap::Parser;
use nix::{
    sched::CloneFlags,
    sys::wait::wait,
    unistd::{close, getgid, getuid, pipe, read, Gid, Group, Pid, Uid, User},
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
        CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNS,
        None,
    )
    .context("Clone failed")?;

    // this parent process sets up user namespace mappings, notifies the child to continue,
    // then waits for the child to exit
    close(reader)?;
    setup_id_maps(child, user_id, group_id).context("Failed to setup id mappings")?;
    close(writer)?;
    let status = wait()?;
    if let nix::sys::wait::WaitStatus::Exited(_, code) = status {
        std::process::exit(code);
    } else {
        bail!("Child process failed");
    }
}

// Represents a range of sub uid/gids
#[derive(Debug)]
struct SubIdRange {
    start: usize,
    count: usize,
}

fn get_sub_id_ranges(
    subid_path: impl AsRef<Path>,
    id: &str,
    name: Option<&str>,
) -> anyhow::Result<Vec<SubIdRange>> {
    let subid_path = subid_path.as_ref();
    Ok(read_to_string(subid_path)
        .context(format!(
            "Failed to read sub id file {}",
            subid_path.display()
        ))?
        .lines() // split the string into an iterator of string slices
        .filter_map(|line| {
            let parts = line.splitn(2, ':').collect::<Vec<_>>();
            if parts.len() != 3 {
                // Not a valid line
                return None;
            }
            if Some(parts[0]) != name || parts[0] != id {
                // Not a line for the desired user/group
                return None;
            }
            if let (Ok(start), Ok(count)) = (parts[1].parse::<usize>(), parts[2].parse::<usize>()) {
                Some(SubIdRange { start, count })
            } else {
                None
            }
        })
        .collect())
}

const ETC_SUBUID: &str = "/etc/subuid";
const ETC_SUBGID: &str = "/etc/subgid";

/// Create new uid/gid mappings for the current user/group
fn setup_id_maps(child: Pid, uid: Uid, gid: Gid) -> anyhow::Result<()> {
    let username = User::from_uid(uid).ok().flatten().map(|user| user.name);
    let uid_string = uid.to_string();
    let subuid_ranges = get_sub_id_ranges(ETC_SUBUID, &uid_string, username.as_deref())?;

    let groupname = Group::from_gid(gid)
        .ok()
        .flatten()
        .map(|group: Group| group.name);
    let gid_string = gid.to_string();
    let subgid_ranges = get_sub_id_ranges(ETC_SUBGID, &gid.to_string(), groupname.as_deref())?;

    let mut uid_args = vec![
        child.to_string(),
        "0".to_string(),
        uid_string,
        "1".to_string(),
    ];
    let mut next_uid = 1;
    for range in subuid_ranges {
        uid_args.push(next_uid.to_string());
        uid_args.push(range.start.to_string());
        uid_args.push(range.count.to_string());
        next_uid += range.count;
    }

    let mut gid_args = vec![
        child.to_string(),
        "0".to_string(),
        gid_string,
        "1".to_string(),
    ];
    let mut next_gid = 1;
    for range in subgid_ranges {
        gid_args.push(next_gid.to_string());
        gid_args.push(range.start.to_string());
        gid_args.push(range.count.to_string());
        next_gid += range.count;
    }

    let status = Command::new("newuidmap").args(uid_args).status()?;
    if !status.success() {
        bail!("Failed to create uid mappings");
    }

    let status = Command::new("newgidmap").args(gid_args).status()?;
    if !status.success() {
        bail!("Failed to create gid mappings");
    }

    Ok(())
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
