//! Module for rpmoci config manifest file
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
use anyhow::Result;
use oci_spec::{
    image::{Arch, ConfigBuilder, ImageConfiguration, ImageConfigurationBuilder, Os},
    OciSpecError,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use url::Url;

#[derive(Debug, Serialize, Default, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
/// Image configuration options
/// Corresponds to the config fields of: https://github.com/opencontainers/image-spec/blob/main/config.md#properties
pub(crate) struct ImageConfig {
    #[serde(default)]
    pub(crate) user: Option<String>,
    #[serde(default)]
    pub(crate) exposed_ports: Vec<String>,
    #[serde(default)]
    pub(crate) envs: HashMap<String, String>,
    #[serde(default)]
    pub(crate) entrypoint: Vec<String>,
    #[serde(default)]
    pub(crate) cmd: Vec<String>,
    #[serde(default)]
    pub(crate) volumes: Vec<String>,
    #[serde(default)]
    pub(crate) labels: HashMap<String, String>,
    #[serde(default)]
    pub(crate) workingdir: Option<String>,
    #[serde(default)]
    pub(crate) stopsignal: Option<String>,
    #[serde(default)]
    pub(crate) author: Option<String>,
}

#[derive(Debug, Serialize, Default, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
/// Configuration on packages to install
pub(crate) struct PackageConfig {
    pub(crate) repositories: Vec<Repository>,
    pub(crate) packages: Vec<String>,
    #[serde(default)]
    pub(crate) gpgkeys: Vec<Url>,
    /// Whether to install documentation files.
    /// Defaults to false, to produce smaller container images.
    #[serde(default = "docs_default")]
    pub(crate) docs: bool,
    /// Whether to include /etc/os-release as a dependency during dependency resolution.
    /// Defaults to true, so that scanning tools can detect
    /// the distro of images produced by rpmoci without users
    /// needing to add the <distro>-release package.
    #[serde(default = "os_release_default")]
    pub(crate) os_release: bool,
}

fn docs_default() -> bool {
    false
}

fn os_release_default() -> bool {
    true
}

/// Configuration file for rpmoci
#[derive(Debug, Serialize, Default, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub(crate) contents: PackageConfig,
    #[serde(default)]
    pub(crate) image: ImageConfig,
}

/// Configuration of a yum/dnf repository
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
#[serde(untagged)]
pub(crate) enum Repository {
    Url(Url),
    Id(String),
    Definition(RepositoryDefinition),
}

/// A repository with a URL + config options
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct RepositoryDefinition {
    id: Option<String>,
    // The base url of the repository
    pub(crate) url: Url,
    /// Additional repository options.
    #[serde(default)]
    pub(crate) options: HashMap<String, String>,
}

impl Repository {
    // A repo id for this repository, inspired by dnf config-manager's behaviour
    pub(crate) fn repo_id(&self) -> String {
        // If the repository is already an id, just return it
        if let Repository::Id(repo_id) = &self {
            return repo_id.to_string();
        }

        // If the repository is a definition, and has an id, return that
        if let Repository::Definition(repo) = &self {
            if let Some(repoid) = &repo.id {
                return repoid.to_string();
            }
        }

        // The repository didn't have an id, so generate one from the url
        let url = match self {
            Repository::Url(url) => url,
            Repository::Definition(repo) => &repo.url,
            Repository::Id(_) => unreachable!(),
        };
        format!(
            "{}_{}",
            url.domain().unwrap_or_default(),
            url.path_segments()
                .map(|segments| segments.collect::<Vec<_>>().join("_"))
                .unwrap_or_default()
        )
    }
}

impl ImageConfig {
    pub(crate) fn to_oci_image_configuration(
        &self,
        cli_labels: HashMap<String, String>,
    ) -> Result<ImageConfiguration, OciSpecError> {
        let ImageConfig {
            user,
            exposed_ports,
            envs,
            entrypoint,
            cmd,
            volumes,
            labels,
            workingdir,
            stopsignal,
            author,
            ..
        } = &self;
        let mut builder = ConfigBuilder::default();
        let mut merged_labels = labels.clone();
        merged_labels.extend(cli_labels);

        // default the PATH variable to /usr/local/bin:/usr/local/sbin:/usr/bin:/usr/sbin:/bin:/sbin
        let mut envs = envs.clone();
        envs.entry("PATH".to_string())
            .or_insert("/usr/local/bin:/usr/local/sbin:/usr/bin:/usr/sbin:/bin:/sbin".to_string());

        builder = builder
            .cmd(cmd.clone())
            .volumes(volumes.clone())
            .entrypoint(entrypoint.clone())
            .env(
                envs.iter()
                    .map(|(k, v)| format!("{}={}", k, v))
                    .collect::<Vec<_>>(),
            )
            .exposed_ports(exposed_ports.clone())
            .labels(merged_labels.clone());
        if let Some(user) = user {
            builder = builder.user(user);
        }
        if let Some(stopsignal) = stopsignal {
            builder = builder.stop_signal(stopsignal);
        }
        if let Some(workingdir) = workingdir {
            builder = builder.working_dir(workingdir);
        }
        let config = builder.build()?;

        let mut builder = ImageConfigurationBuilder::default()
            .config(config)
            .architecture(Arch::Amd64)
            .os(Os::Linux)
            .created(chrono::Utc::now().to_rfc3339());
        if let Some(author) = author {
            builder = builder.author(author);
        }
        builder.build()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::config::ImageConfig;

    use super::Config;

    #[test]
    fn parse_basic() {
        let config = r#"[contents]
        repositories = ["https://packages.microsoft.com/cbl-mariner/2.0/prod/base/x86_64"]
        gpgkeys = [
          "https://raw.githubusercontent.com/microsoft/CBL-Mariner/2.0/SPECS/mariner-repos/MICROSOFT-RPM-GPG-KEY"
        ]
        packages = [
          "skopeo-1.9.*",
          "target/generate-rpm/rpmoci-*.rpm",
          "core-packages-container"
        ]
        
        [image]
        cmd = [ "bash" ]
        "#;
        toml::from_str::<Config>(config).unwrap();
    }

    #[test]
    fn parse_multiple_repository_types() {
        let config = r#"[contents]
        repositories = ["foo-base", "https://packages.microsoft.com/cbl-mariner/2.0/prod/base/x86_64", {id = "foo", url = "https://foo/bar"}]
        gpgkeys = [
          "https://raw.githubusercontent.com/microsoft/CBL-Mariner/2.0/SPECS/mariner-repos/MICROSOFT-RPM-GPG-KEY"
        ]
        packages = [
          "skopeo-1.9.*",
          "target/generate-rpm/rpmoci-*.rpm",
          "core-packages-container"
        ]
        
        [image]
        cmd = [ "bash" ]
        "#;
        toml::from_str::<Config>(config).unwrap();
    }

    #[test]
    fn parse_no_gpgkeys() {
        let config = r#"[contents]
        repositories = ["https://packages.microsoft.com/cbl-mariner/2.0/prod/base/x86_64", {id = "foo", url = "https://foo/bar"}]
        packages = [
          "skopeo-1.9.*",
          "target/generate-rpm/rpmoci-*.rpm",
          "core-packages-container"
        ]
        
        [image]
        cmd = [ "bash" ]
        "#;
        toml::from_str::<Config>(config).unwrap();
    }

    #[test]
    fn parse_inline_repositories() {
        let config = r#"[contents]
        packages = [
          "skopeo-1.9.*",
          "target/generate-rpm/rpmoci-*.rpm",
          "core-packages-container"
        ]
        [[contents.repositories]]
        url = "https://packages.microsoft.com/cbl-mariner/2.0/prod/base/x86_64/"
        options = {includepkgs = "foo,bar"}
        
        [image]
        cmd = [ "bash" ]
        "#;
        toml::from_str::<Config>(config).unwrap();
    }

    #[test]
    fn path_env_defaulting() {
        let config_with_path = r#"
        envs = { PATH = "/usr/bin"}
        "#;
        let config: oci_spec::image::ImageConfiguration =
            toml::from_str::<ImageConfig>(config_with_path)
                .unwrap()
                .to_oci_image_configuration(HashMap::new())
                .unwrap();
        let envs = config.config().as_ref().unwrap().env().as_ref().unwrap();
        assert!(envs.iter().any(|e| e == "PATH=/usr/bin"));
        assert_eq!(envs.len(), 1);

        let config_without_path = r#"
        envs = { FOO = "bar"}
        "#;
        let config: oci_spec::image::ImageConfiguration =
            toml::from_str::<ImageConfig>(config_without_path)
                .unwrap()
                .to_oci_image_configuration(HashMap::new())
                .unwrap();
        let envs = config.config().as_ref().unwrap().env().as_ref().unwrap();
        assert!(envs
            .iter()
            .any(|e| e == "PATH=/usr/local/bin:/usr/local/sbin:/usr/bin:/usr/sbin:/bin:/sbin"));
        assert_eq!(envs.len(), 2);
    }

    #[test]
    fn label_merging() {
        let config_str = r#"
        labels = { "foo.bar" = "baz"}
        "#;
        // No additional labels
        let config: oci_spec::image::ImageConfiguration = toml::from_str::<ImageConfig>(config_str)
            .unwrap()
            .to_oci_image_configuration(HashMap::new())
            .unwrap();
        let labels = config.config().as_ref().unwrap().labels().as_ref().unwrap();
        assert_eq!(labels.get("foo.bar").unwrap(), "baz");
        assert_eq!(labels.len(), 1);

        let extra_labels = [
            ("foo.bar".to_string(), "qux".to_string()),
            ("foo.baz".to_string(), "quux".to_string()),
        ]
        .into_iter()
        .collect();
        let config: oci_spec::image::ImageConfiguration = toml::from_str::<ImageConfig>(config_str)
            .unwrap()
            .to_oci_image_configuration(extra_labels)
            .unwrap();
        let labels = config.config().as_ref().unwrap().labels().as_ref().unwrap();
        assert_eq!(labels.get("foo.bar").unwrap(), "qux");
        assert_eq!(labels.get("foo.baz").unwrap(), "quux");
        assert_eq!(labels.len(), 2);
    }
}
