#!/usr/bin/env python3
"""Run SDK against an isolated real Rust node, signed inclusion and restart."""
import base64, hashlib, json, queue, shutil, subprocess, tempfile, threading, urllib.request
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
def request(base,route,cookie,body=None):
    req=urllib.request.Request(base+route,data=None if body is None else json.dumps(body).encode(),headers={'Content-Type':'application/json','Authorization':'Basic '+base64.b64encode(cookie.encode()).decode()})
    with urllib.request.urlopen(req,timeout=15) as r:return json.load(r)
def start(binary,config,logs):
    p=subprocess.Popen([str(binary),'--config',str(config)],stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
    q=queue.Queue()
    def drain(stream,ready):
        for line in stream:
            logs.append(line.strip())
            if ready and line.startswith('{'):q.put(json.loads(line))
    threading.Thread(target=drain,args=(p.stdout,True),daemon=True).start()
    threading.Thread(target=drain,args=(p.stderr,False),daemon=True).start()
    try:return p,q.get(timeout=30)
    except Exception:p.kill();p.wait();raise

def main():
    report={'passed':False}; logs=[];p=None
    with tempfile.TemporaryDirectory(prefix='lattice-sdk-node-') as directory:
        tmp=Path(directory);(tmp/'bin').mkdir()
        node=Path(shutil.copy2(ROOT.parent/'lattice-node-rs/target/debug/lattice-node',tmp/'bin/node'))
        sdk=Path(shutil.copy2(ROOT/'target/debug/examples/node_workflow',tmp/'bin/sdk'))
        report['binarySHA256']={x.name:hashlib.sha256(x.read_bytes()).hexdigest() for x in [node,sdk]}
        report['harnessSHA256']=hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
        fixture=ROOT.parent/'lattice-rs/crates/lattice-validation/tests/genesis-swift.json'
        report['fixtureSHA256']=hashlib.sha256(fixture.read_bytes()).hexdigest()
        with (tmp/'bootstrap.json').open('w') as out:subprocess.run(['cargo','run','--quiet','--locked','-p','lattice-validation','--example','genesis_volumes','--',str(fixture)],cwd=ROOT.parent/'lattice-rs',stdout=out,check=True)
        config={'directory':'data','identityKey':'identity.key','genesisCid':json.loads(fixture.read_text())[0]['cid'],'bootstrapFiles':['bootstrap.json'],'minRelayFee':0,'listen':'127.0.0.1:0','rpc':'127.0.0.1:0'}
        cfg=tmp/'node.json';cfg.write_text(json.dumps(config))
        identity=json.loads(subprocess.check_output([sdk,'identity'],text=True))
        try:
            p,ready=start(node,cfg,logs);base='http://'+ready['rpc'];cookiepath=tmp/'data/.cookie';cookie=cookiepath.read_text().strip()
            def mine(recipient):
                template=request(base,'/mining/templates',cookie,{'recipients':[{'chainPath':['Nexus'],'address':recipient}]})
                request(base,'/mining/work',cookie,{'workID':template['workID'],'nonce':0})
                return request(base,'/status',cookie)
            mine(identity['sender'])
            tx=json.loads(subprocess.check_output([sdk,'submit',base,str(cookiepath)],text=True))['transactionCID']
            status=mine(identity['sender']);tip=status['tipCID'];assert status['height']=='2',status
            key=identity['sender']+'/123/99'
            proof=request(base,'/api/deposit-state?key='+key,cookie)['proof'];proofpath=tmp/'proof.json';proofpath.write_text(json.dumps(proof))
            def verify():return json.loads(subprocess.check_output([sdk,'verify',base,str(cookiepath),tip,tx,str(proofpath),key],text=True))
            report['beforeRestart']=verify()
            p.terminate();assert p.wait(timeout=15)==0;p=None
            config['bootstrapFiles']=[];cfg.write_text(json.dumps(config))
            p,ready=start(node,cfg,logs);base='http://'+ready['rpc'];report['afterRestart']=verify()
            assert report['beforeRestart']==report['afterRestart']
            report['passed']=True
        finally:
            if p and p.poll() is None:p.terminate();p.wait(timeout=15)
            out=ROOT/'tests/node-integration-evidence.json';out.write_text(json.dumps(report,indent=2)+'\n')
            (ROOT/'tests/node-integration-evidence.log').write_text('\n'.join(logs)+'\n')
    print(json.dumps(report,indent=2))
if __name__=='__main__':main()
