"""Annotate a function's disassembly with sample counts.
usage: python annotate.py exe map raw symbol-substring [top]"""
import sys, re, subprocess, collections
exe, mapf, rawf, sub = sys.argv[1:5]
top = int(sys.argv[5]) if len(sys.argv) > 5 else 40
syms = []
for line in open(mapf, errors='replace'):
    m = re.match(r'\s*[0-9a-f]{4}:[0-9a-f]{8}\s+(\S+)\s+([0-9a-f]{16})\s', line, re.I)
    if m:
        syms.append((int(m.group(2), 16), m.group(1)))
syms.sort()
idx = [i for i, (a, n) in enumerate(syms) if sub in n][0]
start, end = syms[idx][0], syms[idx + 1][0]
raw = collections.Counter()
for line in open(rawf):
    a, c = line.split()[:2]
    a = int(a, 16)
    if start <= a < end:
        raw[a] += int(c)
total = sum(raw.values())
dis = subprocess.run(['llvm-objdump', '-d', '--no-show-raw-insn', '--start-address=0x%x' % start,
                      '--stop-address=0x%x' % end, exe], capture_output=True, text=True).stdout
lines = []
for l in dis.splitlines():
    m = re.match(r'\s*([0-9a-f]+):\s*(.*)', l)
    if m:
        lines.append((int(m.group(1), 16), m.group(2)))
pos = {a: i for i, (a, _) in enumerate(lines)}
print('function samples', total)
shown = set()
for a, c in raw.most_common(top):
    i = pos.get(a)
    if i is None:
        continue
    lo, hi = max(0, i - 6), min(len(lines), i + 2)
    if any(j in shown for j in range(lo, hi)):
        continue
    print('---- %5.1f%%' % (100 * c / total))
    for j in range(lo, hi):
        shown.add(j)
        aa, txt = lines[j]
        print('%6d %x  %s' % (raw.get(aa, 0), aa - start, txt))
