#!/usr/bin/env python3
"""Real Wayland SHM suballocation regression, using the protocol wire format."""
import socket,struct,array,os,pathlib,mmap,itertools
name=os.environ['WAYLAND_DISPLAY'];runtime=pathlib.Path(os.environ['XDG_RUNTIME_DIR'])
assert name, 'set WAYLAND_DISPLAY to the nested compositor socket'
path=name if name.startswith('/') else str(runtime/name)
s=socket.socket(socket.AF_UNIX);s.settimeout(10);s.connect(path);ids=itertools.count(2);pending=b''
def send(obj,opcode,body=b'',fd=None):
 data=struct.pack('II',obj,((len(body)+8)<<16)|opcode)+body
 if fd is None:s.sendall(data)
 else:s.sendmsg([data],[(socket.SOL_SOCKET,socket.SCM_RIGHTS,array.array('i',[fd]))])
def receive():
 global pending
 def read_more():
  data=s.recv(65536)
  if not data:raise RuntimeError('Wayland connection closed before capture completed')
  return data
 while len(pending)<8:pending+=read_more()
 obj,header=struct.unpack_from('II',pending);size=header>>16
 assert size>=8
 while len(pending)<size:pending+=read_more()
 body=pending[8:size];pending=pending[size:]
 if obj==1 and header&0xffff==0:raise RuntimeError('Wayland display error '+repr(body))
 return obj,header&0xffff,body
def string(value):
 raw=value.encode()+b'\0';return struct.pack('I',len(raw))+raw+b'\0'*((-len(raw))%4)
registry=next(ids);send(1,1,struct.pack('I',registry));callback=next(ids);send(1,0,struct.pack('I',callback));globals={}
while True:
 obj,op,data=receive()
 if obj==registry and op==0:
  global_id,length=struct.unpack_from('II',data);interface=data[8:8+length-1].decode();version=struct.unpack_from('I',data,8+((length+3)//4)*4)[0]
  globals.setdefault(interface,(global_id,version))
 if obj==callback and op==0:break
def bind(interface,version):
 global_id,supported=globals[interface];ident=next(ids)
 send(registry,0,struct.pack('I',global_id)+string(interface)+struct.pack('II',min(version,supported),ident));return ident
shm=bind('wl_shm',1);output=bind('wl_output',4);manager=bind('zwlr_screencopy_manager_v1',3)
frame=next(ids);send(manager,0,struct.pack('III',frame,0,output))
while True:
 obj,op,data=receive()
 if obj==frame and op==3:raise RuntimeError('capture refused')
 if obj==frame and op==0:fmt,width,height,stride=struct.unpack('IIII',data)
 if obj==frame and op==6:break
stride=width*4+16;offset=64;suffix=32;length=offset+stride*height+suffix
fd=os.memfd_create('solium-shm-suballocation');os.ftruncate(fd,length);memory=mmap.mmap(fd,length);memory[:]=b'\xa7'*length
pool=next(ids);send(shm,0,struct.pack('Ii',pool,length),fd)
buffer=next(ids);send(pool,0,struct.pack('IiiiiI',buffer,offset,width,height,stride,fmt))
# create_buffer arguments are newid, offset, width, height, stride, format.
send(frame,0,struct.pack('I',buffer))
while True:
 obj,op,data=receive()
 if obj==frame and op==3:raise RuntimeError('copy refused')
 if obj==frame and op==2:break
assert memory[:offset]==b'\xa7'*offset,'capture overwrote the SHM prefix instead of honoring buffer.offset'
for y in range(height):
 begin=offset+y*stride+width*4;end=offset+(y+1)*stride
 assert memory[begin:end]==b'\xa7'*(end-begin),'capture overwrote row padding'
assert memory[-suffix:]==b'\xa7'*suffix,'capture overwrote SHM suffix'
assert any(v!=0xa7 for v in memory[offset:offset+width*4]),'capture left the buffer unfilled'
print('PASS nonzero SHM offset, padded rows, prefix/suffix guards',width,height,flush=True)
s.close();memory.close();os.close(fd)
