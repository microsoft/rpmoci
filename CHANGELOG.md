### Breaking Changes
### Added
### Fixed

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
