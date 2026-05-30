word = "hello"
counts = {}

for c in word:
    # BUG: counts[c] was never initialized, so this raises KeyError.
    counts[c] += 1

print(counts)
