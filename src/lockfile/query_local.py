"""Local package query script."""
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

import rpm
import json
import os


def query_local(local_packages):
    output = []

    for pkg in local_packages:
        fi = os.open(pkg, os.O_RDONLY)
        with open(pkg, "r") as fi:
            ts = rpm.ts()
            headers = ts.hdrFromFdno(fi)
            rpmrequires = headers[rpm.RPMTAG_REQUIRENEVRS]
        output.extend(
                [str(requires) for requires in rpmrequires],
        )
    return json.dumps(output, indent=2)
