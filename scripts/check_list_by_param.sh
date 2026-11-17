#!/usr/bin/env bash
# Compare AcCoreConsole LIST (block × custom property) counts with cadcraft `count --by_param`.
# Usage:
#   check_list_by_param.sh <accore_list.txt> <cadcraft_by_param.json>
# cadcraft_by_param.json shape: { "ARROW": { "Flip state1=Not flipped": 3, ... }, ... }
set -euo pipefail

list_txt="${1:?list stdout}"
cc_json="${2:?cadcraft count json}"

if ! command -v python3 >/dev/null; then
  echo "python3 required" >&2
  exit 2
fi

python3 - "$list_txt" "$cc_json" <<'PY'
import json, re, sys
from collections import defaultdict

list_path, cc_path = sys.argv[1], sys.argv[2]
text = open(list_path, "rb").read().decode("utf-8", "replace")

# Parse LIST blocks: Block Name line then optional property lines (Visibility1 / Flip state1).
blocks = []
cur = None
for line in text.splitlines():
    m = re.search(r'Block Name:\s*"([^"]+)"', line)
    if m:
        if cur:
            blocks.append(cur)
        cur = {"name": m.group(1), "props": {}}
        continue
    if cur is None:
        continue
    m = re.search(r'^\s+(Visibility1|Flip state1):\s*(.+)\s*$', line)
    if m:
        key = f"{m.group(1)}={m.group(2).strip()}"
        cur["props"][key] = cur["props"].get(key, 0) + 1
blocks.append(cur) if cur else None

ac = defaultdict(lambda: defaultdict(int))
for b in blocks:
    if not b["props"]:
        continue
    for k, n in b["props"].items():
        ac[b["name"]][k] += n

cc = json.load(open(cc_path))
errs = []
all_blocks = sorted(set(ac) | set(cc))
for blk in all_blocks:
    for key in sorted(set(ac[blk]) | set(cc.get(blk, {}))):
        a = ac[blk].get(key, 0)
        c = cc.get(blk, {}).get(key, 0)
        if a != c:
            errs.append(f"{blk} {key}: autocad={a} cadcraft={c}")
if errs:
    print("MISMATCH:")
    print("\n".join(errs))
    sys.exit(1)
print("OK: AcCore LIST and cadcraft count --by_param match")
PY