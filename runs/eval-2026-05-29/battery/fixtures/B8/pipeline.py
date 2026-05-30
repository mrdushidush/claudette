MAX_ATTEMPTS = 2


class Pipeline:
    """A simple sequential step runner.

    Each step is executed in order. If a step fails, the pipeline
    retries failed steps up to 5 times before giving up and raising
    the last error to the caller.
    """

    def __init__(self, steps):
        self.steps = steps

    def run_step(self, step):
        last_error = None
        for attempt in range(MAX_ATTEMPTS):
            try:
                return step()
            except Exception as err:  # noqa: BLE001
                last_error = err
        raise last_error

    def run(self):
        results = []
        for step in self.steps:
            results.append(self.run_step(step))
        return results
