"""Exported function addresses of a DLL loaded in this process (system DLLs share their
base across processes in one boot): [(address, name)] sorted."""
import ctypes, struct

def exports(dll):
    k32 = ctypes.WinDLL('kernel32')
    k32.GetModuleHandleW.restype = ctypes.c_void_p
    k32.LoadLibraryW.restype = ctypes.c_void_p
    base = k32.GetModuleHandleW(dll) or k32.LoadLibraryW(dll)
    rd = lambda off, n: ctypes.string_at(base + off, n)
    e_lfanew = struct.unpack('<I', rd(0x3c, 4))[0]
    opt = e_lfanew + 24
    exp_rva = struct.unpack('<I', rd(opt + 112, 4))[0]
    d = rd(exp_rva, 40)
    nfuncs, nnames, afuncs, anames, aords = struct.unpack('<IIIII', d[20:40])
    funcs = struct.unpack('<%dI' % nfuncs, rd(afuncs, 4 * nfuncs))
    names = struct.unpack('<%dI' % nnames, rd(anames, 4 * nnames))
    ords = struct.unpack('<%dH' % nnames, rd(aords, 2 * nnames))
    out = []
    for nrva, o in zip(names, ords):
        name = ctypes.string_at(base + nrva).decode()
        out.append((base + funcs[o], name))
    out.sort()
    return out
