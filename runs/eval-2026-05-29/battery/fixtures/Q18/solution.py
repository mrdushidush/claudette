def add_item(item, bucket=[]):
    """Append `item` to `bucket` and return it.

    If no bucket is supplied, a fresh empty one should be used each time.
    """
    bucket.append(item)
    return bucket
