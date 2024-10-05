//! Module for building layered OCI images
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
use crate::write;
use anyhow::{Context, Result};
use archive::add_pax_extension_header;
use chrono::DateTime;
use derive_builder::{Builder, UninitializedFieldError};
use layer::LayerWriter;
use ocidir::cap_std::fs::Dir;
use ocidir::oci_spec::image::{Descriptor, MediaType};
use ocidir::{new_empty_manifest, Layer, OciDir};
use pyo3::types::{PyAnyMethods, PyModule, PyTuple};
use pyo3::{FromPyObject, Python, ToPyObject};
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

mod archive;
mod layer;

const CREATED_BY: &str = "Created by rpmoci";

#[derive(Debug, Builder)]
#[builder(
    custom_constructor,
    create_empty = empty,
    build_fn(private, name = "fallible_build", error = "DummyError"),
    pattern = "owned"
)]
/// An OCI image builder that can put RPMs into individual layers.
pub struct Imager {
    /// The installroot
    #[builder(setter(custom))]
    filesystem_root: PathBuf,
    /// The OCI directory where the image is being built
    #[builder(setter(custom))]
    oci_dir: OciDir,
    /// The maximum number of layers to create.
    /// The default is 125.
    #[builder(default = "default_max_layers()")]
    max_layers: usize,
    /// The time the image was created.
    /// If not set, the current time is used.
    #[builder(default = "default_creation_time()")]
    creation_time: DateTime<chrono::Utc>,
    /// The compression algorithm to use for the image layers.
    #[builder(default)]
    compression_algorithm: CompressionAlgorithm,
    /// The compression level to use for the image layers.
    ///
    /// The default for zstd is 3, and the default for gzip is 6.
    #[builder(default)]
    compression_level: Option<i32>,
    /// The OCI image configuration.
    #[builder(default)]
    config: ocidir::oci_spec::image::ImageConfiguration,
    /// The OCI image manifest.
    ///
    /// The default is an empty manifest with a media type of "application/vnd.oci.image.manifest.v1+json"
    #[builder(default = "default_manifest()")]
    manifest: ocidir::oci_spec::image::ImageManifest,
    /// The image tag
    #[builder(default = "String::from(\"latest\")", setter(into))]
    tag: String,
    /// Minimum size threshold for packages.
    /// If a package is below this size, it won't be in its own layer
    ///
    /// The default is 5MB
    #[builder(default = "5 * 1024 * 1024")]
    rpm_size_threshold: u64,
}

/// The compression algorithm to use for the image layers.
#[derive(Debug, Default, Clone, Copy)]
pub enum CompressionAlgorithm {
    /// Gzip compression
    #[default]
    Gzip,
    /// Zstandard compression
    Zstd,
}

#[derive(Debug)]
struct DummyError;

impl From<UninitializedFieldError> for DummyError {
    fn from(_ufe: UninitializedFieldError) -> DummyError {
        DummyError
    }
}

impl ImagerBuilder {
    /// Create a new builder with the given paths.
    pub fn build(self) -> Imager {
        self.fallible_build().expect("All fields are initialized")
    }
}

fn default_max_layers() -> usize {
    125
}

fn default_creation_time() -> DateTime<chrono::Utc> {
    chrono::Utc::now()
}

fn default_manifest() -> ocidir::oci_spec::image::ImageManifest {
    new_empty_manifest()
        .media_type(MediaType::ImageManifest)
        .build()
        .unwrap()
}

impl Imager {
    /// Create a new builder with the given paths.
    ///
    /// The OCI directory will be created if it does not exist.
    ///
    /// Errors if the OCI directory cannot be created, or is a non-empty directory
    /// that does not contain an OCI image.
    pub fn with_paths(
        filesystem_root: impl AsRef<Path>,
        oci_dir: impl AsRef<Path>,
    ) -> Result<ImagerBuilder> {
        let filesystem_root = std::path::absolute(filesystem_root)?;
        let oci_dir = oci_dir.as_ref();
        fs::create_dir_all(oci_dir).context(format!(
            "Failed to create OCI image directory `{}`",
            oci_dir.display()
        ))?;
        let dir = Dir::open_ambient_dir(oci_dir, ocidir::cap_std::ambient_authority())
            .context("Failed to open image directory")?;
        let oci_dir = OciDir::ensure(dir)?;

        Ok(ImagerBuilder {
            filesystem_root: Some(filesystem_root),
            oci_dir: Some(oci_dir),
            ..ImagerBuilder::empty()
        })
    }

    /// Build the OCI image, by walking the filesystem and creating layers for each package.
    ///
    /// Returns the descriptor for the image manifest.
    pub fn create_image(self) -> Result<Descriptor> {
        // Determine most popular packages
        let popular_packages = self.most_popular_packages()?;
        // Create a a layer for each package
        let mut package_layers = self.package_layers(&popular_packages)?;
        let path_to_layer_map = path_to_layer_map(popular_packages);
        // Create a catchall layer for any files not in the most popular package layers
        let mut catchall = self.create_layer(CREATED_BY, self.creation_time.timestamp())?;

        // Walk the filesystem and add files to the appropriate layers
        self.walk_filesystem(path_to_layer_map, &mut package_layers, &mut catchall)?;
        // Finalize the image by writing the layers to the OCI image directory
        self.finish(package_layers, catchall)
    }

    fn package_layers<'a>(&'a self, py_pkgs: &[PyPackage]) -> Result<Vec<LayerBuilder<'a>>> {
        // Create a layer for each package
        py_pkgs
            .iter()
            .map(|py_pkg| {
                self.create_layer(
                    format!(
                        "{} for package {}-{}.{}",
                        CREATED_BY, py_pkg.name, py_pkg.evr, py_pkg.arch
                    ),
                    py_pkg.buildtime,
                )
            })
            .collect::<Result<Vec<_>>>()
    }

    /// variation of tar-rs's append_dir_all that:
    /// - works around https://github.com/alexcrichton/tar-rs/issues/102 so that security capabilities are preserved
    /// - emulates tar's `--clamp-mtime` option so that any file/dir/symlink mtimes are no later than a specific value
    /// - supports hardlinks
    /// - adds files to the correct archive layer
    fn walk_filesystem<'a>(
        &self,
        path_to_layer_map: HashMap<PathBuf, usize>,
        package_layers: &mut [LayerBuilder<'a>],
        catchall: &mut LayerBuilder<'a>,
    ) -> Result<()> {
        // Map (dev, inode) -> path for hardlinks
        let mut hardlinks: HashMap<(u64, u64), PathBuf> = HashMap::new();

        for entry in WalkDir::new(&self.filesystem_root)
            .follow_links(false)
            .sort_by_file_name()
            .into_iter()
        {
            let entry = entry?;
            let meta = entry.metadata()?;
            // skip sockets as tar-rs errors when trying to archive them.
            // For comparison, umoci also errors, whereas docker skips them
            if meta.file_type().is_socket() {
                continue;
            }

            let rel_path = pathdiff::diff_paths(entry.path(), &self.filesystem_root)
                .expect("walkdir returns path inside of search root");
            if rel_path == Path::new("") {
                continue;
            }

            // Determine which builder to use
            let wrapped_builder = match path_to_layer_map.get(&rel_path) {
                Some(i) => &mut package_layers[*i],
                None => catchall,
            };
            // Mark the builder as used so that we know to add it to the OCI image
            wrapped_builder.used = true;
            let clamp_mtime = wrapped_builder.clamp_mtime;
            let builder = &mut wrapped_builder.inner;

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

    /// Finalize the image by writing the layers to the OCI image directory
    /// and updating the given manifest and image configuration
    fn finish<'a>(
        &self,
        package_layers: Vec<LayerBuilder<'a>>,
        catchall: LayerBuilder<'a>,
    ) -> Result<Descriptor> {
        write::ok("Writing", "image layers")?;

        let mut manifest = self.manifest.clone();
        let mut config = self.config.clone();

        package_layers
            .into_iter()
            .filter(|b| b.used)
            .try_for_each(|builder| {
                let (layer, created_by) = builder.finish()?;
                self.oci_dir.push_layer_full(
                    &mut manifest,
                    &mut config,
                    layer,
                    Option::<HashMap<String, String>>::None,
                    &created_by,
                    self.creation_time,
                );
                Result::<_, anyhow::Error>::Ok(())
            })?;

        if catchall.used {
            let (layer, created_by) = catchall.finish()?;
            self.oci_dir.push_layer_full(
                &mut manifest,
                &mut config,
                layer,
                Option::<HashMap<String, String>>::None,
                &created_by,
                self.creation_time,
            );
        }

        write::ok("Writing", "image manifest and config")?;
        Ok(self.oci_dir.insert_manifest_and_config(
            manifest,
            config,
            Some(&self.tag),
            ocidir::oci_spec::image::Platform::default(),
        )?)
    }

    fn create_layer(
        &self,
        created_by: impl Into<String>,
        clamp_mtime: i64,
    ) -> Result<LayerBuilder> {
        let mut inner = tar::Builder::new(LayerWriter::new(
            &self.oci_dir,
            self.compression_algorithm,
            self.compression_level,
        )?);
        inner.follow_symlinks(false);
        Ok(LayerBuilder {
            inner,
            created_by: created_by.into(),
            used: false,
            clamp_mtime,
        })
    }

    fn most_popular_packages(&self) -> Result<Vec<PyPackage>> {
        Python::with_gil(|py| {
            // Resolve is a compiled in python module for resolving dependencies
            let _nix_closure_graph = PyModule::from_code_bound(
                py,
                include_str!("nix_closure_graph.py"),
                "nix_closure_graph",
                "nix_closure_graph",
            )?;
            let graph = PyModule::from_code_bound(py, include_str!("graph.py"), "graph", "graph")?;
            let args = PyTuple::new_bound(
                py,
                &[
                    self.filesystem_root.to_object(py),
                    self.max_layers.to_object(py),
                    self.rpm_size_threshold.to_object(py),
                ],
            );
            Ok::<_, anyhow::Error>(
                graph
                    .getattr("most_popular_packages")?
                    .call1(args)?
                    .extract()?,
            )
        })
        .context("Failed to determine layer graph")
    }
}

/// A struct for extracting package information from a hawkey.Package
#[derive(Debug, FromPyObject)]
struct PyPackage {
    name: String,
    evr: String,
    arch: String,
    files: Vec<PathBuf>,
    buildtime: i64,
}

struct LayerBuilder<'a> {
    inner: tar::Builder<LayerWriter<'a>>,
    created_by: String,
    used: bool,
    /// Directories and symlinks in an RPM may have an mtime of the install time.
    /// Whilst rpm respects SOURCE_DATE_EPOCH, we want package layers in independent builds (with different SOURCE_DATE_EPOCHs)
    /// to be identical.
    clamp_mtime: i64,
}

impl<'a> LayerBuilder<'a> {
    fn finish(self) -> Result<(Layer, String)> {
        let layer = self.inner.into_inner()?.complete()?;
        Ok((layer, self.created_by))
    }
}

fn path_to_layer_map(py_pkgs: Vec<PyPackage>) -> HashMap<PathBuf, usize> {
    // Map paths to the index of the layer they belong to
    let mut path_to_layer_idx = HashMap::new();
    for (i, pkg) in py_pkgs.into_iter().enumerate() {
        for file in pkg.files {
            path_to_layer_idx.insert(
                file.strip_prefix("/")
                    .map(|p| p.to_path_buf())
                    .unwrap_or(file),
                i,
            );
        }
    }
    path_to_layer_idx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder() {
        let out = test_temp_dir::test_temp_dir!();
        let cfg = Imager::with_paths("foo", out.as_path_untracked())
            .unwrap()
            .build();
        assert_eq!(cfg.max_layers, 125);
    }
}
