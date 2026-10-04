def parse_csv_line(line):
    """Parse a single CSV line into a list of field strings.

    e.g. parse_csv_line('a,b,c') -> ['a', 'b', 'c']
    """
    return line.split(",")
