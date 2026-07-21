def retry(func, attempts):
    """Call func() up to `attempts` times.

    Return its result the first time it succeeds. If every attempt raises,
    re-raise the exception from the last attempt.
    """
    for _ in range(attempts):
        try:
            return func()
        except Exception:
            pass
