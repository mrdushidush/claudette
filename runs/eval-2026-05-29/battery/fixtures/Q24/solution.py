def word_wrap(text, width):
    """Greedily wrap `text` into lines no longer than `width` characters,
    breaking only between words. A word longer than `width` gets its own line.
    Lines are joined with newlines.

    e.g. word_wrap("the quick brown fox", 9) -> "the quick\nbrown fox".
    """
    raise NotImplementedError
