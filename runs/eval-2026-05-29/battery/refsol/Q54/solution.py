import heapq
from collections import defaultdict


def topological_sort(deps):
    """Order nodes so each comes after everything it depends on.

    Ties are broken alphabetically for determinism. Returns None on a cycle.
    """
    nodes = set(deps)
    for requires in deps.values():
        nodes.update(requires)

    indegree = {n: 0 for n in nodes}
    dependents = defaultdict(list)
    for node, requires in deps.items():
        for dep in requires:
            dependents[dep].append(node)
            indegree[node] += 1

    heap = [n for n in nodes if indegree[n] == 0]
    heapq.heapify(heap)
    result = []
    while heap:
        node = heapq.heappop(heap)
        result.append(node)
        for dependent in dependents[node]:
            indegree[dependent] -= 1
            if indegree[dependent] == 0:
                heapq.heappush(heap, dependent)

    if len(result) != len(nodes):
        return None  # a cycle left some nodes unresolved
    return result
