### Breaking Changes
- Add runtime dependency on sqlite and build-time dependency on sqlite-devel

### Added
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
