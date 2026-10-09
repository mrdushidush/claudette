import time


class Counter:
    """A counter shared across threads."""

    def __init__(self):
        self.value = 0

    def increment(self):
        tmp = self.value
        time.sleep(0)  # allow other threads to run
        self.value = tmp + 1

    def get(self):
        return self.value
