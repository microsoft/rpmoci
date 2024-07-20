# rpmoci

rpmoci builds OCI container images from RPM packages, using [DNF](https://github.com/rpm-software-management/dnf). It's essentially a containerization wrapper around `dnf install --installroot=/some/rootfs PACKAGE [PACKAGE ...]`.

rpmoci features:

 - **deterministic** rpmoci locks RPM dependencies using the package file/lockfile paradigm of bundler/cargo etc and can produce reproducible images with identical digests.
 - **unprivileged** rpmoci can build images in environments without access to a container runtime, and without root access (this relies on the user being able to create [user namespaces](https://www.man7.org/linux/man-pages/man7/user_namespaces.7.html))
 - **small** rpmoci images are built solely from the RPMs you request and their dependencies, so don't contain unnecessary dependencies.

rpmoci is a good fit for containerizing applications - you package your application as an RPM, and then use rpmoci to build a minimal container image from that RPM.

The design of rpmoci is influenced by [apko](https://github.com/chainguard-dev/apko) and [distroless](https://github.com/GoogleContainerTools/distroless) tooling.
rpmoci is also similar to a smaller [`rpm-ostree compose image`](https://coreos.github.io/rpm-ostree/container/#creating-base-images), with a focus on building microservices.


## Installing

rpmoci has a runtime dependency on dnf, so requires a Linux distribution with dnf support.

rpmoci is available to download from crates.io, so you'll need a Rust toolchain. You also need to install the sqlite, python3 and openssl development packages (e.g `sqlite-devel`, `python3-devel` and `openssl-devel` on Fedora and RHEL derivatives). 

Then install rpmoci via cargo:
```bash
cargo install rpmoci
```

## Building
Per the above, you'll need dnf, Rust, python3-devel and openssl-devel installed.

```bash
cargo build
```

### Rootless setup
When rpmoci runs as a non-root user it will automatically attempt to setup a user namespace in which to run.
rpmoci maps the user's uid/gid to root in the user namespace.

It also attempts to map the current user's subuid/subgid range into the user namespace, which is required for rpmoci to be able to create containers from RPMs that contain files owned by a non-root user.

rpmoci requires that at least 999 subuids/subgids are allocated to your user. You can create them per [https://rootlesscontaine.rs/getting-started/common/subuid/](https://rootlesscontaine.rs/getting-started/common/subuid/).

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
url = "https://packages.microsoft.com/cbl-mariner/2.0/prod/base/x86_64/"
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

#### Documentation file

Whether or not documentation files are included in the produced containers can be specified via the `content.docs` boolean field.
By default documentation files are not included, optimizing for image size.


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
url = "https://packages.microsoft.com/cbl-mariner/2.0/prod/base/x86_64/"
id = "foo"
```

### Image configuration

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

#### /etc/os-release

Whether `/etc/os-release` is automatically included as a dependency during resolution, hence installed in the produced image, can be specified via the `content.os_release` boolean field.
This enables SBOM and vulnerability scanning tools to better determine the provenance of packages within the image.
By default this field is enabled.

*The /etc/os-release file can also be included by adding the distro's `<distro>-release` package to the packages array: this field exists to ensure the /etc/os-release file is included by default.*

#### Weak dependencies

rpmoci does not install [weak dependencies](https://docs.fedoraproject.org/en-US/packaging-guidelines/WeakDependencies/#:~:text=Weak%20dependencies%20should%20be%20used%20where%20possible%20to,require%20the%20full%20feature%20set%20of%20the%20package.), optimizing for small container image sizes.

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

#### Lockfiles

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

#### Reproducible builds
rpmoci can produce bitwise reproducible container image builds, assuming that the RPMs can be reproducibly installed (an rpmoci build won't be reproducible if it involves RPMs that have unreproducible post-install scripts for example).
rpmoci attempts to remove sources of non-determinism from the container image, and respects the [SOURCE_DATE_EPOCH](https://reproducible-builds.org/docs/source-date-epoch/) environment variable.

When SOURCE_DATE_EPOCH is not set, the image creation time in the OCI image config is set to the current time. In this scenario rpmoci still removes non-deteministic data from the image, and the build can later be reproduced by setting SOURCE_DATE_EPOCH to the creation time of the image (by converting the timestamp in the image config to seconds since unix epoch). 

This feature is only been tested on Mariner Linux, but should work when rpmoci is run on any Linux distribution that writes the rpmdb as a sqlite database to `/var/lib/rpm/rpmdb.sqlite`.

#### Vendoring

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


#### SBOM support
rpmoci doesn't have native SBOM support, but because it just uses standard OS package functionality SBOM generators like trivy and syft can be used to generate SBOMs for the produced images.

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

### Testing

The tests are run via `cargo test`. The integration tests in `tests/it.rs` run `rpmoci build`,
so must be run either as root, or with user namespace support setup.

The tests use [test-temp-dir](https://docs.rs/crate/test-temp-dir/latest), so you can set 
the `TEST_TEMP_RETAIN` environment variable to `1` so that the test directories are kept around for debugging in `<CARGO_TARGET_DIR>/tests`.
