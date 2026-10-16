# Map Mach-O i386 symbol-stub / pointer addresses to imported symbol names.
# Usage: python3 stubs.py level.i386 > stubs.txt
import struct,sys,subprocess
d=open(sys.argv[1],'rb').read()
ncmds=struct.unpack_from('<I',d,16)[0]
off=28; sects=[]; symoff=None
for i in range(ncmds):
    cmd,sz=struct.unpack_from('<II',d,off)
    if cmd==1:
        nsects=struct.unpack_from('<I',d,off+48)[0]
        so=off+56
        for j in range(nsects):
            sn=d[so:so+16].rstrip(b'\0').decode(); sg=d[so+16:so+32].rstrip(b'\0').decode()
            addr,size,offs,align,reloff,nreloc,flags,r1,r2=struct.unpack_from('<9I',d,so+32)
            sects.append((sg,sn,addr,size,flags,r1,r2)); so+=68
    elif cmd==2: symoff,nsyms,stroff,strsize=struct.unpack_from('<4I',d,off+8)
    elif cmd==0xb: dys=struct.unpack_from('<18I',d,off+8)
    off+=sz
indoff,nind=dys[12],dys[13]
def symname(i):
    n_strx=struct.unpack_from('<I',d,symoff+12*i)[0]
    s=stroff+n_strx; return d[s:d.find(b'\0',s)].decode()
out={}
for sg,sn,addr,size,flags,r1,r2 in sects:
    t=flags&0xff
    if t in (8,6,7): # symbol stubs / pointers
        es = r2 if t==8 else 4
        for k in range(size//es):
            idx=struct.unpack_from('<I',d,indoff+4*(r1+k))[0]
            name = symname(idx) if idx < 0x40000000 else hex(idx)
            out[addr+k*es]=(sn,name)
for a in sorted(out): print(hex(a),out[a][0],out[a][1])
