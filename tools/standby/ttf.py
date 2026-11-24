import struct
class TTF:
    def __init__(s,path):
        d=s.d=open(path,'rb').read()
        n=struct.unpack('>H',d[4:6])[0]; s.t={}
        for i in range(n):
            tag,cs,off,ln=struct.unpack('>4sIII',d[12+16*i:28+16*i]); s.t[tag.decode()]=(off,ln)
        h=s.t['head'][0]; s.upem=struct.unpack('>H',d[h+18:h+20])[0]; s.locfmt=struct.unpack('>h',d[h+50:h+52])[0]
        m=s.t['maxp'][0]; s.ng=struct.unpack('>H',d[m+4:m+6])[0]
        lo=s.t['loca'][0]
        if s.locfmt==0: s.loca=[struct.unpack('>H',d[lo+2*i:lo+2*i+2])[0]*2 for i in range(s.ng+1)]
        else: s.loca=[struct.unpack('>I',d[lo+4*i:lo+4*i+4])[0] for i in range(s.ng+1)]
        hh=s.t['hhea'][0]; nh=struct.unpack('>H',d[hh+34:hh+36])[0]; hm=s.t['hmtx'][0]
        s.adv=[struct.unpack('>H',d[hm+4*min(i,nh-1):hm+4*min(i,nh-1)+2])[0] for i in range(s.ng)]
        s.cmap=s._cmap()
    def _cmap(s):
        d=s.d;c=s.t['cmap'][0];n=struct.unpack('>H',d[c+2:c+4])[0]
        for i in range(n):
            pid,eid,off=struct.unpack('>HHI',d[c+4+8*i:c+12+8*i])
            o=c+off
            if struct.unpack('>H',d[o:o+2])[0]==4:
                sc=struct.unpack('>H',d[o+6:o+8])[0]//2
                ends=struct.unpack('>%dH'%sc,d[o+14:o+14+2*sc]); p=o+16+2*sc
                starts=struct.unpack('>%dH'%sc,d[p:p+2*sc]); p+=2*sc
                deltas=struct.unpack('>%dh'%sc,d[p:p+2*sc]); p+=2*sc
                ro=p; ranges=struct.unpack('>%dH'%sc,d[p:p+2*sc])
                m={}
                for k in range(sc):
                    for ch in range(starts[k],min(ends[k],0x17f)+1):
                        if ranges[k]==0: g=(ch+deltas[k])&0xffff
                        else:
                            a=ro+2*k+ranges[k]+2*(ch-starts[k]); g=struct.unpack('>H',d[a:a+2])[0]
                            if g: g=(g+deltas[k])&0xffff
                        m[chr(ch)]=g
                return m
    def contours(s,g):
        d=s.d;o=s.t['glyf'][0]+s.loca[g]
        if s.loca[g]==s.loca[g+1]: return []
        nc,=struct.unpack('>h',d[o:o+2])
        if nc<0:
            out=[];p=o+10
            while True:
                fl,gi=struct.unpack('>HH',d[p:p+4]);p+=4
                if fl&1: dx,dy=struct.unpack('>hh',d[p:p+4]);p+=4
                else: dx,dy=struct.unpack('>bb',d[p:p+2]);p+=2
                if fl&8: p+=2
                elif fl&0x40: p+=4
                elif fl&0x80: p+=8
                for c in s.contours(gi): out.append([(x+dx,y+dy,on) for x,y,on in c])
                if not fl&0x20: return out
        ends=struct.unpack('>%dH'%nc,d[o+10:o+10+2*nc]); p=o+10+2*nc
        il,=struct.unpack('>H',d[p:p+2]); p+=2+il
        npts=ends[-1]+1; flags=[]
        while len(flags)<npts:
            f=d[p];p+=1;flags.append(f)
            if f&8:
                r=d[p];p+=1;flags+= [f]*r
        def coords(short,same):
            v=0;out=[]
            nonlocal p
            for f in flags:
                if f&short:
                    dv=d[p];p+=1
                    v+= dv if f&same else -dv
                elif not f&same:
                    v+=struct.unpack('>h',d[p:p+2])[0];p+=2
                out.append(v)
            return out
        xs=coords(2,16); ys=coords(4,32)
        res=[];st=0
        for e in ends:
            res.append([(xs[i],ys[i],flags[i]&1) for i in range(st,e+1)]); st=e+1
        return res
