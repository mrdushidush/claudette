def topological_sort(deps):
    """Order nodes so each comes after everything it depends on.

    `deps` maps a node to the list of nodes it depends on (which must come
    before it). Returns a list of all nodes in dependency order, or None if the
    dependencies contain a cycle.
    """
    raise NotImplementedError
