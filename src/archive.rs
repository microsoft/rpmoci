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
use anyhow::{Context, Result};
use std::{
    collections::{hash_map::Entry, HashMap},
    io::Write,
    os::unix::{
        fs::MetadataExt,
        prelude::{FileTypeExt, OsStrExt},
    },
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

// https://mgorny.pl/articles/portability-of-tar-features.html#id25
const PAX_SCHILY_XATTR: &[u8; 13] = b"SCHILY.xattr.";

/// custom implementation of tar-rs's append_dir_all that:
/// - works around https://github.com/alexcrichton/tar-rs/issues/102 so that security capabilities are preserved
/// - emulates tar's `--clamp-mtime` option so that any file/dir/symlink mtimes are no later than a specific value
/// - supports hardlinks
pub(super) fn append_dir_all_with_xattrs(
    builder: &mut tar::Builder<impl Write>,
    src_path: impl AsRef<Path>,
    clamp_mtime: i64,
) -> Result<()> {
    let src_path = src_path.as_ref();
    // Map (dev, inode) -> path for hardlinks
    let mut hardlinks: HashMap<(u64, u64), PathBuf> = HashMap::new();

    for entry in WalkDir::new(src_path)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        let meta = entry.metadata()?;
        // skip sockets as tar-rs errors when trying to archive them.
        // For comparison, umoci also errors, whereas docker skips them
        if meta.file_type().is_socket() {
            continue;
        }

        let rel_path = pathdiff::diff_paths(entry.path(), src_path)
            .expect("walkdir returns path inside of search root");
        if rel_path == Path::new("") {
            continue;
        }

        if entry.file_type().is_symlink() {
            if meta.mtime() > clamp_mtime {
                // Setting the mtime on a symlink is fiddly with tar-rs, so we use filetime to change
                // the mtime before adding the symlink to the tar archive
                let mtime = filetime::FileTime::from_unix_time(clamp_mtime, 0);
                filetime::set_symlink_file_times(entry.path(), mtime, mtime)?;
            }
            add_pax_extension_header(entry.path(), builder)?;
            builder.append_path_with_name(entry.path(), rel_path)?;
        } else if entry.file_type().is_file() || entry.file_type().is_dir() {
            add_pax_extension_header(entry.path(), builder)?;

            // If this is a hardlink, add a link header instead of the file
            // if this isn't the first time we've seen this inode
            if meta.nlink() > 1 {
                match hardlinks.entry((meta.dev(), meta.ino())) {
                    Entry::Occupied(e) => {
                        // Add link header and continue to next entry
                        let mut header = tar::Header::new_gnu();
                        header.set_metadata(&meta);
                        if meta.mtime() > clamp_mtime {
                            header.set_mtime(clamp_mtime as u64);
                        }
                        header.set_entry_type(tar::EntryType::Link);
                        header.set_cksum();
                        builder.append_link(&mut header, &rel_path, e.get())?;
                        continue;
                    }
                    Entry::Vacant(e) => {
                        // This is the first time we've seen this inode
                        e.insert(rel_path.clone());
                    }
                }
            }

            let mut header = tar::Header::new_gnu();
            header.set_size(meta.len());
            header.set_metadata(&meta);
            if meta.mtime() > clamp_mtime {
                header.set_mtime(clamp_mtime as u64);
            }
            if entry.file_type().is_file() {
                builder.append_data(
                    &mut header,
                    rel_path,
                    &mut std::fs::File::open(entry.path())?,
                )?;
            } else {
                builder.append_data(&mut header, rel_path, &mut std::io::empty())?;
            };
        }
    }

    Ok(())
}

// Convert any extended attributes on the specified path to a tar PAX extension header, and add it to the tar archive
fn add_pax_extension_header(
    path: impl AsRef<Path>,
    builder: &mut tar::Builder<impl Write>,
) -> Result<(), anyhow::Error> {
    let path = path.as_ref();
    let xattrs = xattr::list(path)
        .with_context(|| format!("Failed to list xattrs from `{}`", path.display()))?;
    let mut pax_header = tar::Header::new_ustar();
    let mut pax_data = Vec::new();
    for key in xattrs {
        let value = xattr::get(path, &key)
            .with_context(|| {
                format!(
                    "Failed to get xattr `{}` from `{}`",
                    key.to_string_lossy(),
                    path.display()
                )
            })?
            .unwrap_or_default();

        // each entry is "<len> <key>=<value>\n": https://www.ibm.com/docs/en/zos/2.3.0?topic=SSLTBW_2.3.0/com.ibm.zos.v2r3.bpxa500/paxex.html
        let data_len = PAX_SCHILY_XATTR.len() + key.as_bytes().len() + value.len() + 3;
        // Calculate the total length, including the length of the length field
        let mut len_len = 1;
        while data_len + len_len >= 10usize.pow(len_len.try_into().unwrap()) {
            len_len += 1;
        }
        write!(pax_data, "{} ", data_len + len_len)?;
        pax_data.write_all(PAX_SCHILY_XATTR)?;
        pax_data.write_all(key.as_bytes())?;
        pax_data.write_all("=".as_bytes())?;
        pax_data.write_all(&value)?;
        pax_data.write_all("\n".as_bytes())?;
    }
    if !pax_data.is_empty() {
        pax_header.set_size(pax_data.len() as u64);
        pax_header.set_entry_type(tar::EntryType::XHeader);
        pax_header.set_cksum();
        builder.append(&pax_header, &*pax_data)?;
    }
    Ok(())
}
