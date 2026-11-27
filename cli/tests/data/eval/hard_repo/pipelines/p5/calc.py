def error_count(lines):
    return sum(1 for line in lines if line.split()[2] == "WARN")
