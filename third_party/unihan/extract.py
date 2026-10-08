#!/usr/bin/env python3
"""从 Unicode Unihan_Readings.txt 抽出匹配引擎用到的行。

只保留 kMandarin 与 kHanyuPinyin，且码位在 CJK 扩展 A（U+3400–U+4DBF）
或 CJK 基本区（U+4E00–U+9FFF）。数据行原样复制，不改字段内容。

Unicode 18.0.0 的取得方式：

    curl -fsSL -o Unihan.zip https://www.unicode.org/Public/18.0.0/ucd/Unihan.zip
    unzip -p Unihan.zip Unihan_Readings.txt > Unihan_Readings.txt
    python3 extract.py Unihan_Readings.txt kMandarin_kHanyuPinyin.txt

上游 Unihan_Readings.txt 的 SHA-256 必须是
9d39995b5de714e8ce93716ed5d15eaa0792d68e407cdf8a0add2893b8f4150b。
"""

from __future__ import annotations

import hashlib
import sys
from pathlib import Path

UPSTREAM_SHA256 = "9d39995b5de714e8ce93716ed5d15eaa0792d68e407cdf8a0add2893b8f4150b"
FIELDS = {"kMandarin", "kHanyuPinyin"}
RANGES = ((0x3400, 0x4DBF), (0x4E00, 0x9FFF))
NOTE = """\
# Lanwork 从 Unicode 18.0.0 Unihan_Readings.txt 抽出的读音行。
# 上游文件 SHA-256：9d39995b5de714e8ce93716ed5d15eaa0792d68e407cdf8a0add2893b8f4150b
# 只保留 kMandarin 与 kHanyuPinyin，码位限于 U+3400..U+4DBF 与 U+4E00..U+9FFF。
# 数据行原样复制。许可见同目录 LICENSE.txt（Unicode License v3）。
#
"""


def in_coverage(codepoint: int) -> bool:
    return any(start <= codepoint <= end for start, end in RANGES)


def extract(source: str) -> str:
    digest = hashlib.sha256(source.encode("utf-8")).hexdigest()
    if digest != UPSTREAM_SHA256:
        raise SystemExit(f"upstream SHA-256 is {digest}, expected {UPSTREAM_SHA256}")
    lines = source.splitlines()
    header: list[str] = []
    data: list[str] = []
    seen_data = False
    for line in lines:
        if not seen_data and (line == "" or line.startswith("#")):
            if line != "# EOF":
                header.append(line)
            continue
        seen_data = True
        if line == "" or line.startswith("#"):
            continue
        codepoint_text, field, *_rest = line.split("\t")
        if field not in FIELDS or not codepoint_text.startswith("U+"):
            continue
        codepoint = int(codepoint_text[2:], 16)
        if in_coverage(codepoint):
            data.append(line)
    body = NOTE + "\n".join(header) + "\n" + "\n".join(data) + "\n# EOF\n"
    return body


def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit("usage: extract.py Unihan_Readings.txt kMandarin_kHanyuPinyin.txt")
    source = Path(sys.argv[1]).read_text(encoding="utf-8")
    text = extract(source)
    Path(sys.argv[2]).write_text(text, encoding="utf-8", newline="\n")
    print(hashlib.sha256(text.encode("utf-8")).hexdigest())
    print(f"lines {text.count(chr(10))}")


if __name__ == "__main__":
    main()
