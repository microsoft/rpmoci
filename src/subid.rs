//! Function related to user namespace setup
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
use anyhow::{bail, Context, Result};
use nix::unistd::{Gid, Group, Pid, Uid, User};
use std::{
    fs::File,
    io::{self, read_to_string, Read},
    process::Command,
};

use crate::write;

/// Represents a range of sub uid/gids
#[derive(Debug, PartialEq)]
pub struct SubIdRange {
    /// The first id in the range
    pub start: usize,
    /// The number of ids in the range
    pub count: usize,
}

const ETC_SUBUID: &str = "/etc/subuid";
const ETC_SUBGID: &str = "/etc/subgid";

/// Create new uid/gid mappings for the current user/group,
/// using the values in /etc/subuid and /etc/subgid
pub fn setup_id_maps(child: Pid, uid: Uid, gid: Gid) -> anyhow::Result<()> {
    let username = User::from_uid(uid).ok().flatten().map(|user| user.name);
    let uid_string = uid.to_string();
    let (newuidmap_args, subuid_count) = newidmap_args(
        File::open(ETC_SUBUID).context("Failed to open /etc/subuid")?,
        &uid_string,
        username.as_deref(),
        child,
    )
    .context("Failed to read subuids from /etc/subuid")?;

    let groupname = Group::from_gid(gid)
        .ok()
        .flatten()
        .map(|group: Group| group.name);
    let gid_string = gid.to_string();
    let (newgidmap_args, subgid_count) = newidmap_args(
        File::open(ETC_SUBGID).context("Failed to open /etc/subgid")?,
        &gid_string,
        groupname.as_deref(),
        child,
    )
    .context("Failed to read subgids from /etc/subgid")?;

    if subuid_count < 1000 {
        write::error(
            "Error",
            "At least 1000 subuids must be configured for the current user in /etc/subuid",
        )?;
    }
    if subgid_count < 1000 {
        write::error(
            "Error",
            "At least 1000 subgids must be configured for the current group in /etc/subgid",
        )?;
    }
    if subuid_count < 1000 || subgid_count < 1000 {
        bail!("Not enough subids available");
    }

    let status = Command::new("newuidmap").args(newuidmap_args).status()?;
    if !status.success() {
        bail!("Failed to create uid mappings");
    }

    let status = Command::new("newgidmap").args(newgidmap_args).status()?;
    if !status.success() {
        bail!("Failed to create gid mappings");
    }

    Ok(())
}

// Determine the newuidmap/newgidmap arguments to configure sub ids,
// and the number of ids that will be mapped
fn newidmap_args(
    etc_subid: impl Read,
    id: &str,
    name: Option<&str>,
    child: Pid,
) -> Result<(Vec<String>, usize)> {
    let mut args = vec![
        child.to_string(),
        "0".to_string(),
        id.to_string(),
        "1".to_string(),
    ];

    let mut next_id = 1;
    for range in get_sub_id_ranges(etc_subid, id, name)? {
        args.push(next_id.to_string());
        args.push(range.start.to_string());
        args.push(range.count.to_string());
        next_id += range.count;
    }
    Ok((args, next_id))
}

/// Get the subid ranges for a user or group
fn get_sub_id_ranges(
    subid_file: impl Read,
    id: &str,
    name: Option<&str>,
) -> io::Result<Vec<SubIdRange>> {
    Ok(read_to_string(subid_file)?
        .lines() // split the string into an iterator of string slices
        .filter_map(|line| {
            let parts = line.splitn(3, ':').collect::<Vec<_>>();
            if parts.len() != 3 {
                // Not a valid line
                return None;
            }
            if Some(parts[0]) != name && parts[0] != id {
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

#[cfg(test)]
mod tests {

    use nix::unistd::Pid;

    use super::{get_sub_id_ranges, SubIdRange};

    #[test]
    fn test_get_sub_id_ranges() {
        let subid_contents = r#"
# this is a comment
user1:100:500
user2:10:10
user3:1000:65536
user1:1:8
1000:100000:5
        "#;
        assert_eq!(
            get_sub_id_ranges(subid_contents.as_bytes(), "1000", None).unwrap(),
            vec![SubIdRange {
                start: 100000,
                count: 5
            }]
        );
        assert_eq!(
            get_sub_id_ranges(subid_contents.as_bytes(), "1000", Some("user1")).unwrap(),
            vec![
                SubIdRange {
                    start: 100,
                    count: 500
                },
                SubIdRange { start: 1, count: 8 },
                SubIdRange {
                    start: 100000,
                    count: 5
                }
            ]
        );
        assert_eq!(
            get_sub_id_ranges(subid_contents.as_bytes(), "1001", Some("user1")).unwrap(),
            vec![
                SubIdRange {
                    start: 100,
                    count: 500
                },
                SubIdRange { start: 1, count: 8 }
            ]
        );
        assert_eq!(
            get_sub_id_ranges(subid_contents.as_bytes(), "1001", Some("user2")).unwrap(),
            vec![SubIdRange {
                start: 10,
                count: 10
            }]
        );
    }

    #[test]
    fn test_newidmap_args() {
        let subid_contents = r#"
# this is a comment
user1:100:500
user2:10:10
user3:1000:65536
user1:1:8
1000:100000:5
        "#;
        assert_eq!(
            super::newidmap_args(
                subid_contents.as_bytes(),
                "1000",
                Some("user1"),
                Pid::from_raw(1234)
            )
            .unwrap(),
            (
                vec![
                    "1234".to_string(),
                    "0".to_string(),
                    "1000".to_string(),
                    "1".to_string(),
                    "1".to_string(),
                    "100".to_string(),
                    "500".to_string(),
                    "501".to_string(),
                    "1".to_string(),
                    "8".to_string(),
                    "509".to_string(),
                    "100000".to_string(),
                    "5".to_string()
                ],
                514
            )
        );
    }
}
