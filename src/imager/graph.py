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
import os
import dnf
from collections import defaultdict
from nix_closure_graph import make_graph_segment_from_root, graph_popularity_contest


def create_package_graph(root):
    """
    Create a graph of installed packages and their dependencies.
    """
    conf = dnf.conf.Conf()
    cache_dir = os.environ.get("RPMOCI_CACHE_DIR", "")
    if cache_dir:
        conf.cachedir = cache_dir
        conf.logdir = cache_dir

    conf.installroot = root
    base = dnf.Base(conf)
    base.fill_sack()
    query = dnf.query.Query(base.sack)
    installed = query.installed()
    graph = defaultdict(set)

    for pkg in installed:
        for req in pkg.requires:
            providers = installed.filter(provides=req)
            if providers:
                for provider in providers:
                    if pkg.name != provider.name and pkg not in graph[provider]:
                        graph[pkg].add(provider)
    return graph


def remove_cycles(graph):
    """
    Repeatedly remove cycles from a graph until it's a DAG.
    """
    from graphlib import TopologicalSorter, CycleError

    while True:
        try:
            _order = [*TopologicalSorter(graph).static_order()]
            break
        except CycleError as e:
            # Remove a cycle
            graph[e.args[1][1]].remove(e.args[1][0])
    return graph


def most_popular_packages(root, n, size_threshold):
    """
    Return the n most popular packages in the specified installroot
    with a size greater than or equal to the specified threshold.

    """
    lookup = remove_cycles(create_package_graph(root))
    new_graph = {}
    for pkg in lookup.keys():
        if pkg in new_graph:
            continue
        new_graph[pkg] = make_graph_segment_from_root(pkg, lookup)

    most_popular = [
        p
        for (p, _count) in sorted(
            graph_popularity_contest(new_graph).items(),
            key=lambda x: (x[1], x[0]),
            reverse=True,
        )
        if p.size >= size_threshold
    ]
    return most_popular[:n] if len(most_popular) >= n else most_popular


# For testing via `python3 src/lockfile/graph.py`
if __name__ == "__main__":
    pkgs = most_popular_packages("/", 100, 5 * 1024 * 1024)
    import pdb

    pdb.set_trace()
