"""Makes assets/emoji.txt: every emoji that is one picture, with its name,
in the groups and the order Unicode lists them in.

    python3 tools/emoji_list.py /usr/share/unicode/emoji/emoji-test.txt assets/emoji.txt

The first file is Unicode's own list of emoji, which most systems have
(the unicode-data package), and which is also at
https://unicode.org/Public/emoji/latest/emoji-test.txt

Emoji that are several joined together (a family, a flag, a skin tone)
are left out: jog draws one picture for one letter, and those are
several letters each.
"""

import re
import sys


def main():
    source, out = sys.argv[1:3]
    lines = []
    for line in open(source, encoding="utf-8"):
        if line.startswith("# group:"):
            lines.append("# " + line.split(":", 1)[1].strip())
            continue
        found = re.match(r"^([0-9A-F ]+);\s*fully-qualified\s*#\s*(\S+)\s+E[\d.]+\s+(.*)$", line)
        if not found:
            continue
        letters = [int(code, 16) for code in found.group(1).split() if int(code, 16) != 0xFE0F]
        if len(letters) == 1:
            lines.append(found.group(2) + "\t" + found.group(3).strip())
    open(out, "w", encoding="utf-8").write("\n".join(lines) + "\n")
    print(sum(1 for line in lines if not line.startswith("#")), "emoji")


if __name__ == "__main__":
    main()
