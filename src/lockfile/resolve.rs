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
use std::collections::{BTreeSet, HashMap};
use std::env;
use std::ops::Deref;

use anyhow::{Context, Result};
use log::debug;
use pyo3::prelude::*;
use pyo3::types::{IntoPyDict, PyDict, PyString, PyTuple};
use url::Url;

use super::{DnfOutput, LocalPackage, Lockfile};
use crate::config::Config;
use crate::config::Repository;

impl Lockfile {
    /// Perform dependency resolution on the given package specs
    pub(crate) fn resolve(
        pkg_specs: Vec<String>,
        repositories: &[Repository],
        gpgkeys: Vec<Url>,
    ) -> Result<Self> {
        let output = Python::with_gil(|py| {
            // Resolve is a compiled in python module for resolving dependencies
            let resolve =
                PyModule::from_code(py, include_str!("resolve.py"), "resolve", "resolve")?;
            let base = setup_base(py, repositories, &gpgkeys)?;

            let args = PyTuple::new(py, &[base.to_object(py), pkg_specs.to_object(py)]);
            // Run the resolve function, returning a json string, which we shall deserialize.
            let val: String = resolve.getattr("resolve")?.call1(args)?.extract()?;
            Ok::<_, anyhow::Error>(val)
        })
        .context("Failed to resolve dependencies with dnf")?;

        let results: DnfOutput = serde_json::from_str(&output)?;
        Ok(Lockfile {
            pkg_specs,
            packages: results.packages.into_iter().collect(),
            local_packages: results.local_packages.into_iter().collect(),
            repo_gpg_config: results.repo_gpg_config,
            global_key_specs: gpgkeys,
        })
    }

    /// Create a lockfile from a configuration file
    pub fn resolve_from_config(cfg: &Config) -> Result<Self> {
        Self::resolve(
            cfg.contents.packages.clone(),
            &cfg.contents.repositories,
            cfg.contents.gpgkeys.clone(),
        )
    }

    /// Resolve dependencies for local packages only
    pub fn resolve_local_rpms(cfg: &Config) -> Result<BTreeSet<LocalPackage>> {
        // Resolve the dependencies of only the local packages
        let local = cfg
            .contents
            .packages
            .clone()
            .into_iter()
            .filter(|spec| spec.ends_with(".rpm"))
            .collect::<Vec<_>>();
        let lockfile = Self::resolve(
            local,
            &cfg.contents.repositories,
            cfg.contents.gpgkeys.clone(),
        )?;
        Ok(lockfile.local_packages)
    }

    /// Create a lockfile by updating any dependencies in the current lockfile
    pub fn resolve_from_previous(&self, cfg: &Config) -> Result<Self> {
        let requires = cfg
            .contents
            .packages
            .clone()
            .into_iter()
            .filter(|spec| !spec.ends_with(".rpm"))
            .chain(
                self.local_packages
                    .iter()
                    .flat_map(|pkg| pkg.requires.clone()),
            )
            .collect::<Vec<_>>();

        let mut lockfile = Self::resolve(
            requires,
            &cfg.contents.repositories,
            cfg.contents.gpgkeys.clone(),
        )?;
        lockfile.local_packages = self.local_packages.clone();
        lockfile.pkg_specs = cfg.contents.packages.clone();
        Ok(lockfile)
    }
}

/// A wrapper around the dnf.Base object which ensures that plugins are unloaded
pub(crate) struct Base<'a> {
    value: &'a PyAny,
}

impl<'a> Deref for Base<'a> {
    type Target = &'a PyAny;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl<'a> Drop for Base<'a> {
    fn drop(&mut self) {
        // Unload plugins as otherwise dnf can raise an error when we call setup_base again
        // self.value.call_method0("unload_plugins").unwrap();
        // To support Azure Linx (Mariner), don't use unload_plugins
        // as it's not present in dnf 4.8.0
        self.value
            .getattr("_plugins")
            .unwrap()
            .call_method0("_unload")
            .unwrap();
    }
}

/// Initialize the dnf.Base object with the repositories configured in the rpmoci.toml
/// The Base object also initializes and configures any system defined plugins
pub(crate) fn setup_base<'a>(
    py: Python<'a>,
    repositories: &[Repository],
    gpgkeys: &[Url],
) -> Result<Base<'a>> {
    let dnf = PyModule::import(py, "dnf")?;
    let base = dnf.getattr("Base")?.call0()?;
    let conf = base.getattr("conf")?;

    base.call_method0("init_plugins")?;
    base.call_method0("pre_configure_plugins")?;

    // If any repositories were specified by repoid, enable them first
    let existing_repos = repositories
        .iter()
        .filter_map(|repo| {
            if let Repository::Id(repoid) = repo {
                Some(repoid)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    if !existing_repos.is_empty() {
        // Load system repos
        base.call_method0("read_all_repos")?;

        // Disable all repos
        let repos = base.getattr("repos")?;
        repos.call_method0("all")?.call_method0("disable")?;
        // Enable the configured ones
        for repo_id in existing_repos {
            repos
                .get_item(repo_id)
                .map_err(|_| {
                    anyhow::anyhow!(
                        "Repository not found in system config, repoid: '{}'",
                        repo_id
                    )
                })?
                .call_method0("enable")?;
        }
    }

    // Now configure any repositories defined by URL/definition
    for repo in repositories {
        let args = PyTuple::new(
            py,
            &[
                PyString::new(py, &repo.repo_id()).to_object(py),
                conf.to_object(py),
            ],
        );
        match &repo {
            Repository::Url(url) => {
                base.getattr("repos")?.call_method(
                    "add_new_repo",
                    args,
                    Some(repo_kwargs(
                        url,
                        &HashMap::new(),
                        gpgkeys,
                        repo_username(&repo.repo_id()),
                        repo_password(&repo.repo_id()),
                        py,
                    )),
                )?;
            }
            Repository::Id(_) => {}
            Repository::Definition(definition) => {
                base.getattr("repos")?.call_method(
                    "add_new_repo",
                    args,
                    Some(repo_kwargs(
                        &definition.url,
                        &definition.options,
                        gpgkeys,
                        repo_username(&repo.repo_id()),
                        repo_password(&repo.repo_id()),
                        py,
                    )),
                )?;
            }
        }
    }

    base.call_method0("configure_plugins")?;

    base.call_method(
        "fill_sack",
        (),
        Some([("load_system_repo", false)].into_py_dict(py)),
    )?;
    Ok(Base { value: base })
}

fn default_repo_options() -> HashMap<String, String> {
    let mut options = HashMap::new();
    options.insert("gpgcheck".to_string(), "True".to_string());
    options.insert("sslverify".to_string(), "True".to_string());
    options
}

pub(crate) fn repo_kwargs<'p>(
    repo_url: &Url,
    repo_options: &HashMap<String, String>,
    gpgkeys: &[Url],
    username: Option<String>,
    password: Option<String>,
    py: Python<'p>,
) -> &'p PyDict {
    let mut kwargs = Vec::new();
    let mut default_repo_options = default_repo_options();

    // Global keys need to be merged with the repo definition keys in case
    // they are needed for repo metadata verification
    let global_gpgkeys = gpgkeys
        .iter()
        .map(|x| x.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    // If the repo definition specified gpgkey, this option won't be used
    default_repo_options.insert("gpgkey".to_string(), global_gpgkeys.clone());

    kwargs.push((
        "baseurl".to_string(),
        [PyString::new(py, repo_url.as_ref())].to_object(py),
    ));

    for (key, val) in repo_options {
        // If the repo definition specified gpgkey, add the global keys to it
        let val = if key == "gpgkey" {
            (val.to_owned() + " " + &global_gpgkeys).to_object(py)
        } else {
            val.to_object(py)
        };
        kwargs.push((key.to_string(), val));
        default_repo_options.remove(key);
    }

    for (key, val) in &default_repo_options {
        kwargs.push((key.to_string(), val.to_object(py)));
    }

    // If auth is configured via envs, add that here
    if let Some(username) = username {
        debug!("using username from environment");
        kwargs.push(("username".to_string(), username.to_object(py)));
    }
    if let Some(password) = password {
        debug!("using password from environment");
        kwargs.push(("password".to_string(), password.to_object(py)));
    }

    kwargs.into_py_dict(py)
}

fn repo_username(repo_id: &str) -> Option<String> {
    env::var(format!(
        "RPMOCI_{}_HTTP_USERNAME",
        repo_id.to_ascii_uppercase()
    ))
    .ok()
}

fn repo_password(repo_id: &str) -> Option<String> {
    env::var(format!(
        "RPMOCI_{}_HTTP_PASSWORD",
        repo_id.to_ascii_uppercase()
    ))
    .ok()
}
