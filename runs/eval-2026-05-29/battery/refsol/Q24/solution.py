def word_wrap(text, width):
    """Greedily wrap `text` into lines no longer than `width` characters,
    breaking only between words. A word longer than `width` gets its own line.
    Lines are joined with newlines.

    e.g. word_wrap("the quick brown fox", 9) -> "the quick\nbrown fox".
    """
    words = text.split()
    if not words:
        return ""
    lines = []
    current = words[0]
    for word in words[1:]:
        if len(current) + 1 + len(word) <= width:
            current += " " + word
        else:
            lines.append(current)
            current = word
    lines.append(current)
    return "\n".join(lines)
