### Breaking Changes
### Added
### Fixed

## 0.4.0 - 2024-10-02
### Breaking Changes
- Remove rootless support in favour of documenting how to use `unshare` to run in a user namespace.

### Added
- Support for distros that have RPM db in /usr/lib/sysimage/rpm

### Fixed
- Set size in hardlink headers correctly.
  - Fixes integrity failures during this `docker push` 


## 0.3.1 - 2024-07-24
### Fixed
- Preserve hardlinks rather than copying files when building images.

## 0.3.0 - 2024-07-18
### Breaking Changes
- Add runtime dependency on sqlite and build-time dependency on sqlite-devel

### Added
- rpmoci respects the SOURCE_DATE_EPOCH environment variable in order to create reproducible images with identical digests.
- Add rootless support for building images.
  - When run as a non-root user, rpmoci will attempt to run within a user namespace to generate the container image.
- rpmoci automatically adds the OS release package (containing `/etc/os-release`) so created images are easier for other tools to scan.
  - This can be disabled via the `contents.os_release` field

### Fixed

## 0.2.12 - 2023-11-28
### Fixed
- Initial release to crates.io.

## 0.2.11 - 2023-10-26
### Fixed
- Pin oci-layout versions to 1.0.0 rather than using the OCI spec version in `oci_spec` crate.
  - rpmoci 0.2.10 produces invalid OCI images that are not compatible with the OCI spec, as they have an oci-layout version of 1.0.1.

## 0.2.10 - 2023-10-25
### Fixed
- Don't attempt to verify signatures from packages from repositories that have `gpgcheck` disabled.

## 0.2.9 - 2023-08-31
### Fixed
- When running an unlocked `build` with a lockfile present check that all dependencies are compatible.
- Set labels from the CLI correctly, regressed in 0.2.8.
- Ignore "rpmlib" dependencies when resolving RPMs with `rpmoci update --from-lockfile`

## 0.2.8 - 2023-08-15

### Fixed
Set a sensible default for the PATH variable correctly.

## 0.2.7 - 2023-08-15

### Fixed
rpmoci preserves capabilities when building images.

## 0.2.6 - 2023-07-27

### Added
Add support for dnf plugins to be used whilst resolving RPMs.

## 0.2.5 - 2023-05-27

Initial open source release.
