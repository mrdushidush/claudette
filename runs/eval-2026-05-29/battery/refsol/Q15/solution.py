def retry(func, attempts):
    """Call func() up to `attempts` times.

    Return its result the first time it succeeds. If every attempt raises,
    re-raise the exception from the last attempt.
    """
    last_exc = None
    for _ in range(attempts):
        try:
            return func()
        except Exception as exc:
            last_exc = exc
    raise last_exc
