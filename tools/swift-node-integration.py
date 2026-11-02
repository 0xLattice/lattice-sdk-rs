#!/usr/bin/env python3
"""Same SDK workflow against unchanged pinned Swift daemon, loopback only."""
import hashlib,json,os,runpy,shutil,socket,subprocess,tempfile,time
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
request=runpy.run_path(str(ROOT/'tools/node-integration.py'))['request']
def port():
    with socket.socket() as s:s.bind(('127.0.0.1',0));return s.getsockname()[1]
def main():
    source=ROOT.parent/'lattice-node';pin='3df9e0a786c7b103a3da88d1c3881efc81706255'
    assert subprocess.check_output(['git','rev-parse','HEAD'],cwd=source,text=True).strip()==pin
    assert not subprocess.check_output(['git','status','--porcelain','--untracked-files=no'],cwd=source,text=True).strip()
    report={'passed':False,'swiftRevision':pin,'harnessSHA256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest()};p=None
    with tempfile.TemporaryDirectory(prefix='lattice-sdk-swift-') as directory:
        tmp=Path(directory);binary=Path(shutil.copy2('/tmp/lattice-node-mixed-build/debug/lattice-node',tmp/'node'));sdk=Path(shutil.copy2(ROOT/'target/debug/examples/node_workflow',tmp/'sdk'));miner=Path(shutil.copy2(ROOT.parent/'lattice-rs/target/debug/examples/mixed_node_signer',tmp/'miner'))
        report['binarySHA256']={x.name:hashlib.sha256(x.read_bytes()).hexdigest() for x in [binary,sdk,miner]}
        cookiepath=tmp/'data/.cookie';rpc=port();base=f'http://127.0.0.1:{rpc}'
        env=dict(os.environ,LATTICE_FORBID_DEFAULT_PEERS='1',LATTICE_TEST_FEE='1')
        command=[str(binary),'--data-directory',str(tmp/'data'),'--identity-key',str(tmp/'identity'),'--listen-port',str(port()),'--rpc-port',str(rpc),'--websocket-port','0','--no-default-peers','--peer-search-interval','0','--provider-walk-sessions','0']
        def start():
            child=subprocess.Popen(command,stdout=log,stderr=subprocess.STDOUT,env=env)
            for _ in range(300):
                assert child.poll() is None,'Swift exited'
                try:
                    if cookiepath.exists() and request(base,'/status',cookiepath.read_text().strip()):return child
                except OSError:pass
                time.sleep(.1)
            child.kill();child.wait();raise RuntimeError('Swift startup timeout')
        with (tmp/'node.log').open('w') as log:
            try:
                p=start();cookie=cookiepath.read_text().strip();identity=json.loads(subprocess.check_output([sdk,'identity'],text=True))
                def mine():
                    template=request(base,'/mining/templates',cookie,{'recipients':[{'chainPath':['Nexus'],'address':identity['sender']}]})
                    path=tmp/'template.json';path.write_text(json.dumps(template));work=json.loads(subprocess.check_output([miner,'--mine',path],text=True))
                    result=request(base,'/mining/work',cookie,{'workID':template['workID'],'nonce':work['nonce']});assert result['accepted'],result
                    return result['tipCID']
                mine();tx=json.loads(subprocess.check_output([sdk,'submit',base,cookiepath],env=env,text=True))['transactionCID'];tip=mine()
                key=identity['sender']+'/123/99';proof=request(base,'/api/deposit-state?key='+key,cookie)['proof'];proofpath=tmp/'proof.json';proofpath.write_text(json.dumps(proof))
                def verify():return json.loads(subprocess.check_output([sdk,'verify',base,cookiepath,tip,tx,proofpath,key],text=True))
                report['beforeRestart']=verify();p.terminate();p.wait(timeout=30);p=None
                p=start();report['afterRestart']=verify();assert report['beforeRestart']==report['afterRestart'];report['passed']=True
            finally:
                if p and p.poll() is None:p.terminate();p.wait(timeout=30)
                log.flush();(ROOT/'tests/swift-node-integration-evidence.log').write_text((tmp/'node.log').read_text().replace(str(tmp), '<fixture-directory>'))
                (ROOT/'tests/swift-node-integration-evidence.json').write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report,indent=2))
if __name__=='__main__':main()
