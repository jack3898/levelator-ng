# Usage: python3 annotate.py level.i386.asm level.i386 level.ann.asm
#   level.i386.asm = `llvm-objdump -d --no-show-raw-insn --x86-asm-syntax=intel level.i386`
# Annotate i386 Mach-O PIC disassembly: resolve [reg+imm] after `call L; L: pop reg`
import re,sys,struct
asm_path, bin_path, out_path = sys.argv[1:4]
d=open(bin_path,'rb').read()
ncmds=struct.unpack('<I',d[16:20])[0]; off=28; secs=[]
for i in range(ncmds):
    cmd,sz=struct.unpack('<II',d[off:off+8])
    if cmd==1:
        nsect=struct.unpack('<I',d[off+48:off+52])[0]; so=off+56
        for j in range(nsect):
            sn=d[so:so+16].rstrip(b'\0').decode(); addr,size,foff=struct.unpack('<3I',d[so+32:so+44])
            secs.append((sn,addr,size,foff)); so+=68
    off+=sz
def rd(a,n):
    for sn,ad,sz,fo in secs:
        if ad<=a<ad+sz and fo: return d[fo+a-ad:fo+a-ad+n], sn
    return None, None
def describe(a):
    b,sn=rd(a,64)
    if b is None: return None
    if sn=='__cstring':
        s=b.split(b'\0')[0]; return 'str "%s"'%s.decode('latin1').replace('\n','\\n')[:60]
    if sn in ('__literal8',):
        return 'f64 %r'%struct.unpack('<d',b[:8])[0]
    if sn in ('__literal4',):
        return 'f32 %r / u32 0x%x'%(struct.unpack('<f',b[:4])[0],struct.unpack('<I',b[:4])[0])
    if sn=='__const':
        return 'const f64 %r | u32 0x%x'%(struct.unpack('<d',b[:8])[0],struct.unpack('<I',b[:4])[0])
    if sn=='__pointers':
        p=struct.unpack('<I',b[:4])[0]; return 'ptr->0x%x'%p
    return sn
lines=open(asm_path).read().splitlines()
base={}  # reg->value
out=[]
pending=None
ins_re=re.compile(r'^\s*([0-9a-f]+):\s*\t(\S+)\s*(.*)$')
for ln in lines:
    m=ins_re.match(ln)
    if not m:
        if ln.endswith('>:'): base={}
        out.append(ln); continue
    addr=int(m.group(1),16); mn=m.group(2); ops=m.group(3)
    if mn=='call':
        t=re.match(r'0x([0-9a-f]+)',ops)
        if t: pending=int(t.group(1),16)
    elif mn=='pop' and pending==addr:
        base[ops.strip()]=addr; pending=None
    else:
        pending=None if mn!='call' else pending
    if mn=='ret': base={}
    note=''
    for r,v in base.items():
        for mm in re.finditer(r'\[%s ?([+-]) ?(0x[0-9a-f]+)\]'%r, ops):
            a=v+int(mm.group(2),16)*(1 if mm.group(1)=='+' else -1)
            ds=describe(a)
            note+=' ; 0x%x %s'%(a,ds or '')
    out.append(ln+note)
open(out_path,'w').write('\n'.join(out))
