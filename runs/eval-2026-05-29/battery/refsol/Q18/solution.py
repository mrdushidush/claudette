def add_item(item, bucket=None):
    """Append `item` to `bucket` and return it.

    If no bucket is supplied, a fresh empty one is used each time.
    """
    if bucket is None:
        bucket = []
    bucket.append(item)
    return bucket
