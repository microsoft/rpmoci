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
use std::{io::Write, os::unix::prelude::OsStrExt, path::Path};

// https://mgorny.pl/articles/portability-of-tar-features.html#id25
const PAX_SCHILY_XATTR: &[u8; 13] = b"SCHILY.xattr.";

// Convert any extended attributes on the specified path to a tar PAX extension header, and add it to the tar archive
pub(crate) fn add_pax_extension_header(
    path: impl AsRef<Path>,
    builder: &mut tar::Builder<impl Write>,
) -> Result<(), anyhow::Error> {
    let path = path.as_ref();
    let xattrs = xattr::list(path)
        .with_context(|| format!("Failed to list xattrs from `{}`", path.display()))?;
    let mut pax_header = tar::Header::new_gnu();
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
