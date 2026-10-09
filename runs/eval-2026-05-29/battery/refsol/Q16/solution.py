def paginate(items, page_size):
    """Split `items` into a list of pages, each with at most page_size items."""
    if page_size <= 0:
        raise ValueError("page_size must be positive")
    return [items[i:i + page_size] for i in range(0, len(items), page_size)]
