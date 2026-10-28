#!/usr/bin/env python3
"""Checks a smoke-test log (app/tests/draw_smoke_test.tscn): fails on script errors, on a
`key=false` that isn't expected, or when the run stopped early. Usage: check_smoke.py LOG"""
import re
import sys

# Checks whose healthy value is false (state after an action, not a pass/fail result).
EXPECTED_FALSE = {"dirty", "after_delete", "editor_visible", "mode_after", "home_visible_after_reopen"}
# The run's last line; missing means it crashed or hung.
LAST = "SMOKE clean_svg_render="

log = open(sys.argv[1], encoding="utf-8", errors="replace").read().splitlines()
smoke = [line for line in log if line.startswith("SMOKE")]
problems = [line for line in log if "SCRIPT ERROR" in line or "Parse Error" in line]
for line in smoke:
    # "reopened=… home_visible=false" is the canvas showing again: expected.
    line = line.replace("strokes=6 home_visible=false", "home_visible_after_reopen=false")
    for key in re.findall(r"(\w+)=false", line):
        if key not in EXPECTED_FALSE:
            problems.append(f"{key}=false in: {line}")
if not any(line.startswith(LAST) for line in smoke):
    problems.append("the run did not finish (no clean_svg_render line)")
print(f"{len(smoke)} smoke lines")
for p in problems:
    print("FAIL:", p)
sys.exit(1 if problems else 0)
