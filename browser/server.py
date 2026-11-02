#!/usr/bin/env python3
"""Local same-origin browser transport oracle; no public service or credentials."""
import base64,hashlib,http.server,json,pathlib,struct,time,urllib.parse
ROOT=pathlib.Path(__file__).resolve().parent
stats={'redirectTargets':0,'delayed':0,'bodyDelayed':0,'abortObserved':0,'authorized':0,'posts':0}
class Handler(http.server.SimpleHTTPRequestHandler):
    def __init__(self,*args,**kwargs):super().__init__(*args,directory=str(ROOT),**kwargs)
    def log_message(self,*args):pass
    def send(self,data,content_type='application/json'):
        self.send_response(200);self.send_header('Content-Type',content_type);self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
    def do_POST(self):
        body=self.rfile.read(int(self.headers.get('Content-Length','0')))
        assert json.loads(body)=={'browser':'test'}
        stats['posts']+=1;self.send(b'{"transactionCID":"test-cid"}')
    def do_GET(self):
        path=urllib.parse.urlsplit(self.path).path
        if path=='/stats':return self.send(json.dumps(stats).encode())
        if path.startswith('/redirect/'):
            self.send_response(302);self.send_header('Location','/redirect-target');self.end_headers();return
        if path=='/redirect-target':stats['redirectTargets']+=1;return self.send(b'{}')
        if path.startswith('/volume/'):
            data=b'\xa0';cid=('b'+base64.b32encode(b'\x01\x71\x12\x20'+hashlib.sha256(data).digest()).decode().rstrip('=').lower()).encode()
            archive=struct.pack('>H',1)+struct.pack('>H',len(cid))+cid+struct.pack('>I',len(data))+data
            return self.send(archive,'application/vnd.lattice.volume')
        if path.startswith('/normal/'):
            if self.headers.get('Authorization')=='Bearer browser-test':stats['authorized']+=1
            return self.send(b'{"chain":["root"]}')
        if path.startswith('/oversize/'):
            self.send_response(200);self.send_header('Content-Type','application/json');self.end_headers();self.wfile.write(b'{"padding":"'+b'x'*10000+b'"}');return
        if path.startswith('/delayed/') or path.startswith('/slow-body/'):
            body=path.startswith('/slow-body/');stats['bodyDelayed' if body else 'delayed']+=1
            try:
                if body:
                    self.send_response(200);self.send_header('Content-Type','application/json');self.end_headers();self.wfile.write(b'{');self.wfile.flush()
                time.sleep(.3)
                if not body:self.send_response(200);self.end_headers()
                for _ in range(32):self.wfile.write(b'x'*32768);self.wfile.flush();time.sleep(.005)
            except (BrokenPipeError,ConnectionResetError):stats['abortObserved']+=1
            return
        return super().do_GET()
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
print(server.server_address[1],flush=True)
server.serve_forever()
