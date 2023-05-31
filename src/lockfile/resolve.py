"""A dependency resolver for rpmoci"""
# Copyright (C) Microsoft Corporation.
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License
# along with this program.  If not, see <https://www.gnu.org/licenses/>.
from dnf.i18n import _
import dnf
import hawkey
import itertools
import json
import glob


def resolve(base, packages):
    """Resolves packages.
    base needs to be a dnf.Base() object that has had repos configured and fill_sack called.
    packages is an array of requested package specifications"""
    pkgs = itertools.chain.from_iterable(
        [get_packages(base, pkg_spec) for pkg_spec in packages]
    )
    goal = hawkey.Goal(base.sack)
    for pkg in pkgs:
        goal.install(pkg)

    if not goal.run():
        msg = dnf.util._format_resolve_problems(goal.problem_rules())
        raise dnf.exceptions.DepsolveError(msg)

    resolved_pkgs = goal.list_installs()
    repo_gpg_info = {}
    # Collect GPG keys for this repository
    for pkg in resolved_pkgs:
        if pkg.repoid != hawkey.CMDLINE_REPO_NAME and pkg.repoid not in repo_gpg_info:
            repo_gpg_info[pkg.repoid] = {
                "gpgcheck": pkg.repo.gpgcheck,
                "keys": retrieve_keys(pkg.repo),
            }

    output = {
        "packages": [
            pkg_to_dict(pkg)
            for pkg in resolved_pkgs
            if pkg.repoid != hawkey.CMDLINE_REPO_NAME
        ],
        "local_packages": [
            {
                "name": pkg.name,
                "requires": [str(requires) for requires in pkg.requires],
            }
            for pkg in resolved_pkgs
            if pkg.repoid == hawkey.CMDLINE_REPO_NAME
        ],
        "repo_gpg_config": repo_gpg_info,
    }
    return json.dumps(output, indent=2)


def get_packages(base, pkg_spec):
    """Find packages matching given spec."""
    if pkg_spec.endswith(".rpm"):
        # Local RPM file
        pkgfilter = base.add_remote_rpms(glob.glob(pkg_spec))
        query = base.sack.query().filterm(pkg=pkgfilter)
    else:
        subj = dnf.subject.Subject(pkg_spec)
        query = subj.get_best_query(base.sack)
        query = query.available()
        query = query.filterm(latest_per_arch_by_priority=True)

    pkgs = query.run()
    if not pkgs:
        msg = "No packages available for spec '%s'" % pkg_spec
        raise dnf.exceptions.DepsolveError(msg)
    return pkgs


def retrieve_keys(repo):
    raw_keys = []
    if repo.gpgcheck:
        for keyurl in repo.gpgkey:
            for key in dnf.crypto.retrieve(keyurl, repo):
                raw_keys.append(key.raw_key.decode())
    return raw_keys


def pkg_to_dict(pkg):
    return {
        "name": pkg.name,
        "evr": pkg.evr,
        "checksum": chksum_to_dict(pkg.chksum),
        "repoid": pkg.repoid,
    }


def chksum_to_dict(chksum):
    if chksum[0] == hawkey.CHKSUM_MD5:  # Devskim: ignore DS126858
        algo = "md5"  # Devskim: ignore DS126858
    elif chksum[0] == hawkey.CHKSUM_SHA1:  # Devskim: ignore DS126858
        algo = "sha1"  # Devskim: ignore DS126858
    elif chksum[0] == hawkey.CHKSUM_SHA256:
        algo = "sha256"
    elif chksum[0] == hawkey.CHKSUM_SHA384:
        algo = "sha384"
    elif chksum[0] == hawkey.CHKSUM_SHA512:
        algo = "sha512"
    else:
        raise dnf.exceptions.Error("Unknown checksum value %d" % chksum[0])
    return {"algorithm": algo, "checksum": chksum[1].hex()}
