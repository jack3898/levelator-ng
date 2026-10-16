# Split a Mach-O universal ("fat") binary into its per-architecture slices.
# Usage: python3 split_fat.py level   -> level.i386, level.ppc
import struct, sys

path = sys.argv[1]
d = open(path, "rb").read()
names = {7: "i386", 18: "ppc", 0x01000007: "x86_64"}
for i in range(struct.unpack(">I", d[4:8])[0]):
    cputype, _, off, size, _ = struct.unpack(">5I", d[8 + 20 * i : 28 + 20 * i])
    out = f"{path}.{names.get(cputype, hex(cputype))}"
    open(out, "wb").write(d[off : off + size])
    print(out, size)
