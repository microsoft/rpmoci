//! OCI image related functionality
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
use flate2::{write::GzEncoder, Compression};
use oci_spec::image::{Descriptor, DescriptorBuilder, MediaType, OciLayout, OciLayoutBuilder};
use serde::Serialize;
use std::{
    collections::{hash_map::Entry, HashMap},
    fs,
    io::Write,
    os::unix::{
        fs::MetadataExt,
        prelude::{FileTypeExt, OsStrExt},
    },
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;
use walkdir::WalkDir;

use crate::sha256_writer::Sha256Writer;

const OCI_LAYOUT_PATH: &str = "oci-layout";
// OCI layout version: https://github.com/opencontainers/image-spec/blob/main/image-layout.md#oci-layout-file
// Fixed at 1.0.0 until changes to the layout format are made, when changes are made the new version will be
// taken from the OCI spec version that makes the changes.
const OCI_LAYOUT_VERSION: &str = "1.0.0";

/// Initialize an [OCI image directory](https://github.com/opencontainers/image-spec/blob/main/image-layout.md) if required
///
/// If the directory doesn't exist, it will be created.
/// If the directory exists and is a valid OCI layout directory, return Ok.
/// Returns an error if the directory exists already and is not
/// an OCI image directory
pub(crate) fn init_image_directory(layout: impl AsRef<Path>) -> Result<(), anyhow::Error> {
    // If path exists, check whether it's a valid OCI image directory
    if layout.as_ref().exists() {
        match fs::read_dir(layout.as_ref()) {
            Ok(dir) => {
                // if the directory exists but is empty, then initialize it
                if dir.count() == 0 {
                    init_dir(layout.as_ref())?;
                }

                match OciLayout::from_file(layout.as_ref().join(OCI_LAYOUT_PATH)) {
                    Ok(oci_layout) => {
                        if oci_layout.image_layout_version() != OCI_LAYOUT_VERSION {
                            bail!(
                                "Unsupported image layout version found: {}. rpmoci only supports oci-layout version {}",
                                oci_layout.image_layout_version(),
                                OCI_LAYOUT_VERSION
                            )
                        }
                    }
                    Err(e) => {
                        bail!(
                            "Failed to read oci-layout file in directory: {}. Error: {}",
                            layout.as_ref().display(),
                            e
                        )
                    }
                }
            }
            Err(e) => {
                return Err(e).context(format!("Failed to read `{}`", layout.as_ref().display()))
            }
        }
    } else {
        // Path doesn't exist so just create a new OCI image directory
        fs::create_dir_all(layout.as_ref()).context(format!(
            "Failed to create OCI image directory `{}`",
            layout.as_ref().display()
        ))?;

        init_dir(layout.as_ref())?;
    }
    Ok(())
}

/// Create blobs/sha256, index.json and oci-layout file in a directory
fn init_dir(layout: impl AsRef<Path>) -> Result<(), anyhow::Error> {
    // Create blobs directory
    let blobs_dir = layout.as_ref().join("blobs").join("sha256");
    fs::create_dir_all(&blobs_dir).context(format!(
        "Failed to create blobs/sha256 directory `{}`",
        blobs_dir.display()
    ))?;

    // create oci-layout file
    let oci_layout = OciLayoutBuilder::default()
        .image_layout_version(OCI_LAYOUT_VERSION)
        .build()?;
    let oci_layout_path = layout.as_ref().join(OCI_LAYOUT_PATH);
    oci_layout.to_file(&oci_layout_path).context(format!(
        "Failed to write to oci-layout file `{}`",
        oci_layout_path.display()
    ))?;

    // create image index
    let index = oci_spec::image::ImageIndexBuilder::default()
        .manifests(Vec::new())
        .schema_version(2u32)
        .build()?;
    let index_path = layout.as_ref().join("index.json");
    let index_file = std::fs::File::create(&index_path).context(format!(
        "Failed to create index.json file `{}`",
        index_path.display()
    ))?;
    serde_json::to_writer(index_file, &index).context(format!(
        "Failed to write to index.json file `{}`",
        index_path.display()
    ))?;

    Ok(())
}

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

/// Create a root filesystem image layer from a directory on disk.
/// The blob is written to the specified OCI layour directory.
///
/// Returns a Descriptor of the blob, and the [`diff_id`](https://github.com/opencontainers/image-spec/blob/main/config.md#layer-diffid) of the layer :
pub(crate) fn create_image_layer(
    rootfs_path: impl AsRef<Path>,
    layout_path: impl AsRef<Path>,
    creation_time: chrono::DateTime<chrono::Utc>,
) -> Result<(Descriptor, String)> {
    // We need to determine the sha256 hash of the compressed and uncompresssed blob.
    // The former for the blob id and the latter for the rootfs diff id which we need to include in the config blob.
    let enc = GzEncoder::new(
        Sha256Writer::new(NamedTempFile::new()?),
        Compression::fast(),
    );
    let mut tar = tar::Builder::new(Sha256Writer::new(enc));
    tar.follow_symlinks(false);
    append_dir_all_with_xattrs(&mut tar, rootfs_path.as_ref(), creation_time.timestamp())
        .context("failed to archive root filesystem")?;
    let (diff_id_sha, gz) = tar.into_inner()?.finish();
    let (blob_digest, mut tmp_file) = gz.finish().context("failed to finish enc")?.finish();
    tmp_file.flush()?;

    let blob_path = layout_path.as_ref().join("blobs/sha256").join(&blob_digest);

    let (blob, tmp_path) = tmp_file.keep()?;
    let size: i64 = blob.metadata()?.len().try_into()?;
    // May fail if tempfile on different filesystem
    if fs::rename(&tmp_path, &blob_path).is_err() {
        fs::copy(&tmp_path, &blob_path).context(format!(
            "Failed to write image layer `{}`",
            blob_path.display()
        ))?;
    }

    Ok((
        DescriptorBuilder::default()
            .digest(format!("sha256:{}", blob_digest))
            .media_type(MediaType::ImageLayerGzip)
            .size(size)
            .build()?,
        format!("sha256:{}", diff_id_sha),
    ))
}

/// Write a json object with the specified media type to the specified
/// OCI layout directory
pub(crate) fn write_json_blob<T>(
    value: &T,
    media_type: MediaType,
    layout_path: impl AsRef<Path>,
) -> Result<Descriptor>
where
    T: ?Sized + Serialize,
{
    let mut writer = Sha256Writer::new(NamedTempFile::new()?);
    serde_json::to_writer(&mut writer, value)
        .context("Failed to write to blob to temporary file")?;
    writer.flush()?;
    let (blob_sha, tmp_file) = writer.finish();
    let blob_path = layout_path.as_ref().join("blobs/sha256").join(&blob_sha);

    let (blob, tmp_path) = tmp_file.keep()?;
    let size: i64 = blob.metadata()?.len().try_into()?;
    // May file if tempfile on different filesystem
    if fs::rename(&tmp_path, &blob_path).is_err() {
        fs::copy(&tmp_path, &blob_path)
            .context(format!("Failed to write blob `{}`", blob_path.display()))?;
    }

    Ok(DescriptorBuilder::default()
        .digest(format!("sha256:{}", blob_sha))
        .media_type(media_type)
        .size(size)
        .build()?)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use oci_spec::image::OciLayout;

    use super::init_image_directory;

    #[test]
    fn test_init() {
        let test_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/init/actual");
        let _ = std::fs::remove_dir_all(&test_dir);
        init_image_directory(&test_dir).unwrap();

        assert_eq!(
            OciLayout::from_file(test_dir.join("oci-layout"))
                .unwrap()
                .image_layout_version(),
            "1.0.0"
        );
    }
}
