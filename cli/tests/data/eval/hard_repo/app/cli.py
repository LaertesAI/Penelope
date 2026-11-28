import sys

from app.export import export_lines
from app.report import report_lines


def main(argv):
    if not argv or argv[0] == "all":
        for line in report_lines() + export_lines():
            print(line)
        return 0
    print("unknown command: %s" % argv[0])
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
