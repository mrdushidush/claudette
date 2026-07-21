def parse_csv_line(line):
    """Parse a single CSV line into a list of field strings.

    Handles quoted fields (which may contain commas) and doubled "" quote
    escapes inside quoted fields.
    e.g. parse_csv_line('a,"b,c",d') -> ['a', 'b,c', 'd']
    """
    fields = []
    field = []
    in_quotes = False
    i = 0
    n = len(line)
    while i < n:
        c = line[i]
        if in_quotes:
            if c == '"':
                if i + 1 < n and line[i + 1] == '"':
                    field.append('"')
                    i += 2
                    continue
                in_quotes = False
                i += 1
                continue
            field.append(c)
            i += 1
        else:
            if c == '"':
                in_quotes = True
                i += 1
            elif c == ",":
                fields.append("".join(field))
                field = []
                i += 1
            else:
                field.append(c)
                i += 1
    fields.append("".join(field))
    return fields
