import os

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def numbers():
    with open(os.path.join(ROOT, "data", "numbers.txt")) as f:
        return [int(float(line)) for line in f if line.strip()]
