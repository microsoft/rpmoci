"""RPM downloader module."""
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
import shutil
import sys
from dnf.i18n import _
import dnf
from dnf.cli.progress import MultiFileProgressMeter


def download(base, packages, directory):
    """Downloads packages.
    Parameters:
    - base needs to be a dnf.Base() object that has had repos configured and fill_sack called.
    packages is an array of requested package specifications
    - packages is a list of {name, evr, checksum} dicts.
    - directory, where to copy the RPMs to
    """
    pkgs = [get_package(base, p[0], p[1], p[2]) for p in packages]
    base.download_packages(pkgs, MultiFileProgressMeter(fo=sys.stdout))
    for pkg in pkgs:
        shutil.copy(pkg.localPkg(), directory)


def get_package(base, name, evr, checksum):
    """Find packages matching given spec."""
    pkgs = base.sack.query().filter(name=name, evr=evr).run()

    if not pkgs:
        msg = (
            "Package could no longer be found in repositories. Name: '%s', evr: '%s'"
            % name,
            evr,
        )
        raise dnf.exceptions.DepsolveError(msg)
    return pkgs[0]
