def paginate(items, page_size):
    """Split `items` into a list of pages, each with at most page_size items."""
    pages = []
    page = []
    for item in items:
        page.append(item)
        if len(page) == page_size:
            pages.append(page)
            page = []
    pages.append(page)
    return pages
