#!/usr/bin/env python3
"""A lightweight Lua 5.1 sanity checker for the OCWow addon.

No Lua interpreter is required: this strips comments and strings, then verifies
that block keywords are balanced. It catches the most common class of syntax
error (unbalanced `end`/`until`, unterminated strings or long comments) but is
not a full parser.

Usage:
    python3 tools/check_lua.py addon/OCWow/*.lua
"""

import re
import sys

OPENERS = {"function", "if", "for", "while", "repeat"}
# `do` belongs to a preceding `for`/`while`; only a standalone `do` opens a block.
DO_OWNERS = {"for", "while"}


def strip_comments_and_strings(src: str) -> str:
    out = []
    i, n = 0, len(src)
    while i < n:
        c = src[i]

        if c == "-" and i + 1 < n and src[i + 1] == "-":
            j = i + 2
            m = re.match(r"\[(=*)\[", src[j:])
            if m:
                level = len(m.group(1))
                close = "]" + "=" * level + "]"
                k = src.find(close, j)
                if k == -1:
                    raise SyntaxError("unterminated long comment")
                i = k + len(close)
                continue
            k = src.find("\n", j)
            i = n if k == -1 else k + 1
            continue

        if c in "\"'":
            quote = c
            j = i + 1
            while j < n and src[j] != quote:
                if src[j] == "\\":
                    j += 2
                    continue
                if src[j] == "\n":
                    raise SyntaxError(f"unterminated string at offset {i}")
                j += 1
            if j >= n:
                raise SyntaxError(f"unterminated string at offset {i}")
            i = j + 1
            out.append('""')
            continue

        if c == "[":
            m = re.match(r"\[(=*)\[", src[i:])
            if m:
                level = len(m.group(1))
                close = "]" + "=" * level + "]"
                k = src.find(close, i + 2 + level)
                if k == -1:
                    raise SyntaxError(f"unterminated long string at offset {i}")
                i = k + len(close)
                out.append('""')
                continue

        out.append(c)
        i += 1

    return "".join(out)


def check(path: str) -> list:
    with open(path, encoding="utf-8") as handle:
        src = handle.read()

    try:
        code = strip_comments_and_strings(src)
    except SyntaxError as err:
        return [f"{path}: {err}"]

    tokens = re.findall(r"[A-Za-z_][A-Za-z0-9_]*", code)

    stack = []
    errors = []
    for index, token in enumerate(tokens):
        if token in OPENERS:
            stack.append(token)
        elif token == "do":
            if stack and stack[-1] in DO_OWNERS:
                pass  # `for ... do` / `while ... do`
            else:
                stack.append("do")
        elif token == "end":
            if not stack:
                errors.append("unexpected `end`")
                break
            opener = stack.pop()
            if opener == "repeat":
                errors.append("`end` closing a `repeat` block")
        elif token == "until":
            if not stack or stack[-1] != "repeat":
                errors.append("`until` without a matching `repeat`")
                break
            stack.pop()

    if not errors and stack:
        errors.append(f"unclosed block(s): {', '.join(reversed(stack))}")

    return [f"{path}: {e}" for e in errors]


def main(paths: list) -> int:
    failures = []
    for path in paths:
        errors = check(path)
        if errors:
            failures.extend(errors)
            for error in errors:
                print("FAIL", error)
        else:
            print(f"ok   {path}")
    if failures:
        print(f"\n{len(failures)} problem(s) found")
        return 1
    print("\nall files balanced")
    return 0


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(2)
    sys.exit(main(sys.argv[1:]))
