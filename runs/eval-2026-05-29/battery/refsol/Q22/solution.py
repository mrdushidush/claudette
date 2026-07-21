import threading


class Counter:
    """A counter that is safe to increment from many threads."""

    def __init__(self):
        self.value = 0
        self._lock = threading.Lock()

    def increment(self):
        with self._lock:
            self.value += 1

    def get(self):
        with self._lock:
            return self.value
