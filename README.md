# rpmoci

rpmoci builds OCI container images from RPM packages, using [DNF](https://github.com/rpm-software-management/dnf). It's essentially a containerization wrapper around `dnf install --installroot=/some/rootfs PACKAGE [PACKAGE ...]`.

rpmoci features:

 - **deterministic** rpmoci locks RPM dependencies using the package file/lockfile paradigm of bundler/cargo etc and supports vendoring of RPMs for later rebuilds.
 - **no container runtime required** rpmoci can build images in environments without docker access.
 - **small** rpmoci images are built solely from the RPMs you request and their dependencies, so don't contain unnecessary packages.

The design of rpmoci is influenced by [apko](https://github.com/chainguard-dev/apko) and [distroless](https://github.com/GoogleContainerTools/distroless) tooling.

## Installing

rpmoci has a runtime dependency on dnf, so requires a Linux distribution with dnf support.

rpmoci is available to download from crates.io, so you'll need a Rust toolchain. You also need to install the python3 and openssl development packages (e.g `python3-devel` and `openssl-devel` on Fedora and RHEL derivatives). 

Then install rpmoci via cargo:
```bash
cargo install rpmoci
```

## Building
Per the above, you'll need dnf, Rust, python3-devel and openssl-devel installed.

```bash
cargo build
```

## Getting started
You need to create an rpmoci.toml file. An example is:

```toml
[contents] # specifies the RPMs that comprise the image
repositories = [ "mariner-official-base" ]
packages = [
  "tini"
]

[image] # specifies image configuration such as entrypoint, ports, cmd, etc.
entrypoint = [ "tini", "--" ]
```

This configures rpmoci to install `tini` and its dependencies from the mariner-official-base repository, and configures the image entrypoint to use `tini`.


This can then be built into an image:
```bash
sudo rpmoci build --image tini --tag my-first-rpmoci-image
```

The image will be created in a OCI layout directory called `tini`.
rpmoci doesn't handle image distribution - users are expected to use tools like [oras](https://oras.land/) or [skopeo](https://github.com/containers/skopeo) to push the image to a registry.

A lockfile, `rpmoci.lock`, will be created so you can re-run the build later and get the same packages.
*assuming they still exist in the specified repository... rpmoci supports vendoring RPMs so you can repeat locked builds without relying on that*

## Reference
### Package Specification
#### Repository configuration
The repository section defines where RPMs are sourced from.

In the getting started example, the repository was specified by its repo id on the running system.
It is also possible to fully specify the repository in `rpmoci.toml`, if you want to create a portable `rpmoci.toml` that can say, build the same image when running on Fedora/Ubuntu/Mariner.

Repositories can be specified via their base URL
```toml
[contents]
repositories = ["https://packages.microsoft.com/cbl-mariner/2.0/prod/base/x86_64"]
```

or defined with additional configuration options in the package manifest file (`rpmoci.toml` by default, can be specified via `-f FILE` on CLI)
```toml
[[contents.repositories]]
url = https://packages.microsoft.com/cbl-mariner/2.0/prod/base/x86_64/
options = { includepkgs = "foo,bar" }
```

By default the `gpgcheck` and `sslverify` are enabled - these can be disabled via the `options` field.

All system repos are ignored, other than those explicitly specified via repo id.
dnf plugins are supported, but rpmoci doesn't support specifying plugin configuration.

#### Package configuration

Package specifications are added under the `contents.packages` key. Both local and remote packages are supported

```toml
[contents]
repositories = ["https://packages.microsoft.com/cbl-mariner/2.0/prod/base/x86_64"]
packages = [
  "postgreqsl", # a package from the above repository
  "path/to/local.rpm", # a local RPM
]
```

#### GPG key configuration
GPG keys can be configued via the repository options or the `gpgkeys` field

```toml
[contents]
repositories = ["https://packages.microsoft.com/cbl-mariner/2.0/prod/base/x86_64"]
gpgkeys = [
  "https://raw.githubusercontent.com/microsoft/CBL-Mariner/2.0/SPECS/mariner-repos/MICROSOFT-RPM-GPG-KEY"
]
packages = [
  "postgresql"
]
```

When building images the package signatures will be verified using the configured GPG keys, except for local packages or packages from repositories where `gpgcheck` has explicitly been disabled.

#### Authenticated RPM repositories
To use a repository that requires HTTP basic authentication, specify an `id` for the repository in the toml file,
and define the environment variables `RPMOCI_<id>_HTTP_USERNAME` and `RPMOCI_<id>_HTTP_PASSWORD` to be the HTTP authentication credentials, where `<id>` is the uppercased repo id.

E.g with the following configuration you would need to define the environment variables `RPMOCI_FOO_HTTP_USERNAME` and `RPMOCI_FOO_HTTP_PASSWORD`:
```toml
[[contents.repositories]]
url = https://packages.microsoft.com/cbl-mariner/2.0/prod/base/x86_64/
id = "foo"
```

#### Documentation

Whether or not documentation files are included in the produced containers can be specified via the `content.docs` boolean field.
By default documentation files are not included, optimizing for image size.

### Image building

Running `rpmoci build --image foo --tag bar` will build a container image in OCI format.

```bash
$ rpmoci build --image foo --tag bar
...
$ cat foo/index.json | jq
{
  "schemaVersion": 2,
  "manifests": [
    {
      "mediaType": "application/vnd.oci.image.manifest.v1+json",
      "digest": "sha256:1ad8cc1866d359e4e2ecb37fcc96759815540f06cb468811dcb9b8aac51da90d",
      "size": 350,
      "annotations": {
        "org.opencontainers.image.ref.name": "bar"
      }
    }
  ]
}
```

This image can then be copied using OCI tools such as skopeo or oras. E.g to copy to a local docker daemon:
```bash
$ skopeo copy oci:foo:bar docker-daemon:foo:bar
Getting image source signatures
Copying blob 77b582c1f09c done
Copying config 577bea913f done
Writing manifest to image destination
Storing signatures
```

#### Image configuration

Additional [image configuration](https://github.com/opencontainers/image-spec/blob/main/config.md#properties) can be specified under the `image` key:

```toml
[contents]
repositories = ["https://packages.microsoft.com/cbl-mariner/2.0/prod/base/x86_64"]
gpgkeys = [
  "https://raw.githubusercontent.com/microsoft/CBL-Mariner/2.0/SPECS/mariner-repos/MICROSOFT-RPM-GPG-KEY"
]
packages = [
  "postgresql"
]
[image]
entrypoint = ["tini", "--"]
cmd = [ "foo" ]
exposed_ports = ["8080/tcp"]

[image.envs]
RUST_BACKTRACE = "1"
RUST_LOG = "hyper=info"
```

The `config` section of the OCI image spec, linked above, maps to the image section in `rpmoci.toml`.
For example to specify image labels you can use the `image.labels` section and to specify image environment variables use `image.envs`.

The PATH environment variable is set to `/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin` by default, but can be overridden via the `image.envs` field.

### Lockfiles

rpmoci uses DNF to produce a lockfile of the build. This can be used to subsequently repeat the build with `rpmoci build --locked`.

A lockfile can be created or updated by running `rpmoci update`:

```bash
$ rpmoci update
Adding filesystem 1.1-10.cm2
Adding grep 3.7-2.cm2
Adding openssl 1.1.1k-17.cm2
Adding libgcc 11.2.0-2.cm2
Adding postgresql 14.2-2.cm2
Adding libxml2 2.9.14-1.cm2
Adding ncurses-libs 6.3-1.cm2
Adding pcre 8.45-2.cm2
Adding pcre-libs 8.45-2.cm2
Adding glibc 2.35-2.cm2
Adding bash 5.1.8-1.cm2
Adding libsepol 3.2-2.cm2
Adding libcap 2.60-1.cm2
Adding krb5 1.19.3-1.cm2
Adding openldap 2.4.57-7.cm2
Adding coreutils 8.32-3.cm2
Adding postgresql-libs 14.2-2.cm2
Adding libselinux 3.2-1.cm2
Adding openssl-libs 1.1.1k-17.cm2
Adding readline 8.1-1.cm2
Adding tzdata 2022a-1.cm2
Adding xz-libs 5.2.5-1.cm2
Adding libstdc++ 11.2.0-2.cm2
Adding zlib 1.2.12-1.cm2
Adding e2fsprogs-libs 1.46.5-1.cm2
Adding gmp 6.2.1-2.cm2
Adding bzip2-libs 1.0.8-1.cm2
```

### Vendoring

RPMs can be vendored to a folder using `rpmoci vendor`. A vendor folder can be used during a build to avoid contacting package repositories.

```bash
$ rpmoci vendor --out-dir vendor
$ ls vendor
ls vendor
031e779a7ce198662c5b266d7b0dfc9eece9c0c888a657b6a9bb7731df0096d0.rpm  8ea3d75dbb48fa12eacf732af89a600bd97709b55f88d98fe129c13ab254de95.rpm
...
$ rpmoci build --image foo --tag bar --vendor-dir vendor
```
*Vendor directories from different invocations of `rpmoci vendor` should be kept isolated, as rpmoci currently attempts to install all RPMs from the vendor directory.*


### SBOM support
rpmoci doesn't have native SBOM support, but because it just uses standard OS package functionality SBOM generators like trivy and syft can be used to generate SBOMs for the produced images.

*For these tools to detect the Linux distribution correctly you may need to install the `<distro>-release` package in the image.*

## Developing

rpmoci is written in Rust and currently resolves RPMs using DNF via an embedded Python module.

It has buildtime dependencies on `python3-devel` and `openssl-devel`.

After checking out the project you can do
```bash
cargo run
```
to run it, or build an RPM using [cargo-generate-rpm](https://github.com/cat-in-136/cargo-generate-rpm):
```bash
cargo generate-rpm
```
