"""Poor man's sampling profiler for Windows: runs an exe, samples its main thread's RIP
via SuspendThread/GetThreadContext, maps addresses to functions with a linker /MAP file.
usage: python sampler.py prof.exe prof.map args...
"""
import ctypes, ctypes.wintypes as wt, subprocess, sys, time, bisect, collections, re, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import exports

k32 = ctypes.WinDLL('kernel32', use_last_error=True)
psapi = ctypes.WinDLL('psapi', use_last_error=True)

TH32CS_SNAPTHREAD = 0x4
THREAD_ALL = 0x001F03FF

class THREADENTRY32(ctypes.Structure):
    _fields_ = [('dwSize', wt.DWORD), ('cntUsage', wt.DWORD), ('th32ThreadID', wt.DWORD),
                ('th32OwnerProcessID', wt.DWORD), ('tpBasePri', wt.LONG), ('tpDeltaPri', wt.LONG),
                ('dwFlags', wt.DWORD)]

class M128A(ctypes.Structure):
    _fields_ = [('Low', ctypes.c_ulonglong), ('High', ctypes.c_longlong)]

class CONTEXT(ctypes.Structure):
    _pack_ = 16
    _fields_ = [('P1Home', ctypes.c_ulonglong), ('P2Home', ctypes.c_ulonglong), ('P3Home', ctypes.c_ulonglong),
                ('P4Home', ctypes.c_ulonglong), ('P5Home', ctypes.c_ulonglong), ('P6Home', ctypes.c_ulonglong),
                ('ContextFlags', wt.DWORD), ('MxCsr', wt.DWORD),
                ('SegCs', wt.WORD), ('SegDs', wt.WORD), ('SegEs', wt.WORD), ('SegFs', wt.WORD), ('SegGs', wt.WORD), ('SegSs', wt.WORD),
                ('EFlags', wt.DWORD),
                ('Dr0', ctypes.c_ulonglong), ('Dr1', ctypes.c_ulonglong), ('Dr2', ctypes.c_ulonglong), ('Dr3', ctypes.c_ulonglong),
                ('Dr6', ctypes.c_ulonglong), ('Dr7', ctypes.c_ulonglong),
                ('Rax', ctypes.c_ulonglong), ('Rcx', ctypes.c_ulonglong), ('Rdx', ctypes.c_ulonglong), ('Rbx', ctypes.c_ulonglong),
                ('Rsp', ctypes.c_ulonglong), ('Rbp', ctypes.c_ulonglong), ('Rsi', ctypes.c_ulonglong), ('Rdi', ctypes.c_ulonglong),
                ('R8', ctypes.c_ulonglong), ('R9', ctypes.c_ulonglong), ('R10', ctypes.c_ulonglong), ('R11', ctypes.c_ulonglong),
                ('R12', ctypes.c_ulonglong), ('R13', ctypes.c_ulonglong), ('R14', ctypes.c_ulonglong), ('R15', ctypes.c_ulonglong),
                ('Rip', ctypes.c_ulonglong),
                ('FltSave', ctypes.c_byte * 512),
                ('VectorRegister', M128A * 26), ('VectorControl', ctypes.c_ulonglong),
                ('DebugControl', ctypes.c_ulonglong), ('LastBranchToRip', ctypes.c_ulonglong),
                ('LastBranchFromRip', ctypes.c_ulonglong), ('LastExceptionToRip', ctypes.c_ulonglong),
                ('LastExceptionFromRip', ctypes.c_ulonglong)]

CONTEXT_CONTROL = 0x00100001

def load_map(path):
    syms = []
    pref = 0x140000000
    for line in open(path, errors='replace'):
        m = re.match(r'\s*[0-9a-f]{4}:[0-9a-f]{8}\s+(\S+)\s+([0-9a-f]{16})\s', line, re.I)
        if m:
            syms.append((int(m.group(2), 16), m.group(1)))
        m2 = re.search(r'Preferred load address is ([0-9a-f]+)', line, re.I)
        if m2:
            pref = int(m2.group(1), 16)
    syms.sort()
    return pref, syms

def short(name):
    # Rust v0 mangled names: keep the readable identifiers.
    parts = re.findall(r'\d+([A-Za-z_][A-Za-z0-9_]*)', name)
    parts = [p for p in parts if not re.match(r'^Cs[A-Za-z0-9]+$', p)]
    return '::'.join(parts[-3:]) if parts else name

def main():
    exe, mapf = sys.argv[1], sys.argv[2]
    pref, syms = load_map(mapf)
    addrs = [a for a, _ in syms]
    p = subprocess.Popen([exe] + sys.argv[3:], stdout=subprocess.PIPE)
    time.sleep(0.05)
    # main thread: the first thread of the process
    snap = k32.CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)
    te = THREADENTRY32(); te.dwSize = ctypes.sizeof(THREADENTRY32)
    tid = None
    ok = k32.Thread32First(snap, ctypes.byref(te))
    while ok:
        if te.th32OwnerProcessID == p.pid:
            tid = te.th32ThreadID
            break
        ok = k32.Thread32Next(snap, ctypes.byref(te))
    k32.CloseHandle(snap)
    hproc = k32.OpenProcess(0x0410 | 0x0010, False, p.pid)
    mods = (ctypes.c_void_p * 1)(); needed = wt.DWORD()
    psapi.EnumProcessModulesEx(hproc, mods, ctypes.sizeof(mods), ctypes.byref(needed), 3)
    base = mods[0]
    # Other modules (system DLLs), to name samples outside the program.
    class MODINFO(ctypes.Structure):
        _fields_ = [('base', ctypes.c_void_p), ('size', wt.DWORD), ('entry', ctypes.c_void_p)]
    allmods = (ctypes.c_void_p * 512)()
    psapi.EnumProcessModulesEx(hproc, allmods, ctypes.sizeof(allmods), ctypes.byref(needed), 3)
    ranges = []
    for m in allmods[: needed.value // ctypes.sizeof(ctypes.c_void_p)]:
        info = MODINFO(); name = ctypes.create_unicode_buffer(260)
        if m and psapi.GetModuleInformation(hproc, ctypes.c_void_p(m), ctypes.byref(info), ctypes.sizeof(info)):
            psapi.GetModuleBaseNameW(hproc, ctypes.c_void_p(m), name, 260)
            ranges.append((m, m + info.size, name.value))
    k32.OpenThread.restype = wt.HANDLE
    ht = k32.OpenThread(THREAD_ALL, False, tid)
    ctx = CONTEXT(); ctx.ContextFlags = CONTEXT_CONTROL
    counts = collections.Counter(); n = 0; raw = collections.Counter(); dll_exports = {}
    while p.poll() is None:
        if k32.SuspendThread(ht) == 0xFFFFFFFF:
            break
        if k32.GetThreadContext(ht, ctypes.byref(ctx)):
            a = ctx.Rip - base + pref
            i = bisect.bisect_right(addrs, a) - 1
            if base <= ctx.Rip < base + (ranges[0][1] - ranges[0][0] if ranges else 1 << 30) and i >= 0:
                counts[short(syms[i][1])] += 1
            else:
                mod = next((nm for lo, hi, nm in ranges if lo <= ctx.Rip < hi), 'other')
                name = '[' + mod + ']'
                ex = dll_exports.get(mod.lower())
                if ex is None and mod != 'other':
                    try:
                        ex = exports.exports(mod)
                    except Exception:
                        ex = []
                    dll_exports[mod.lower()] = ex
                if ex:
                    k = bisect.bisect_right([a for a, _ in ex], ctx.Rip) - 1
                    if k >= 0:
                        name += ex[k][1] + '~'
                # Outside the program (a system leaf such as memcpy): name the caller too,
                # from the return address on top of the stack (right for leaf routines).
                # (Heuristic: the first word on the stack's top 64 that points into the
                # program, which for system routines is usually the caller's return address.)
                words = (ctypes.c_ulonglong * 64)(); got = ctypes.c_size_t()
                if k32.ReadProcessMemory(hproc, ctypes.c_void_p(ctx.Rsp), words, 8 * 64, ctypes.byref(got)):
                    top = base + (ranges[0][1] - ranges[0][0] if ranges else 0)
                    for w in words:
                        if base <= w < top:
                            j = bisect.bisect_right(addrs, w - base + pref) - 1
                            if j >= 0:
                                name += ' <- ' + short(syms[j][1])
                            break
                counts[name] += 1
            n += 1
            raw[a] += 1
        k32.ResumeThread(ht)
        time.sleep(0.0005)
    out = p.stdout.read().decode(errors='replace')
    print(out.strip())
    print(f'{n} samples')
    for name, c in counts.most_common(25):
        print(f'{100 * c / n:5.1f}%  {name}')
    with open(sys.argv[0] + '.raw', 'w') as fh:
        for a, c in raw.most_common():
            fh.write('%x %d %s' % (a, c, chr(10)))

main()
