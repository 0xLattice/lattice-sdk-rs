// Run after npm ci --ignore-scripts in the pinned SDK checkout.
import {execFileSync} from 'node:child_process';
import {readFileSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
import {createHash} from 'node:crypto';
const root=resolve(process.argv[2]??'../lattice-sdk');
const revision='3a1be07318f12b85f147e0a5ea55ae68a224e780';
if(execFileSync('git',['rev-parse','HEAD'],{cwd:root,encoding:'utf8'}).trim()!==revision) throw Error('source pin mismatch');
if(execFileSync('git',['status','--porcelain','--untracked-files=no'],{cwd:root,encoding:'utf8'}).trim()) throw Error('source checkout modified');
execFileSync('npm',['run','build'],{cwd:root,stdio:'inherit'});
const {declaredEndpointURL}=await import(pathToFileURL(resolve(root,'packages/client/dist/endpoints.js')));
const hosts=['127.0.0.1','127.1','0x7f000001','10.1.1.1','172.16.1.1','172.32.1.1','192.168.0.1','192.0.2.1','100.64.0.1','100.128.0.1','8.8.8.8','198.18.0.1','203.0.113.1','224.0.0.1','[::1]','[::ffff:127.0.0.1]','[::ffff:808:808]','[64:ff9b::a00:1]','[64:ff9b::808:808]','[2002:7f00:1::]','[2002:808:808::]','[2001:db8::1]','[2001:4860:4860::8888]','[fc00::1]','localhost','node.local','node.internal','node.home.arpa','node.example','NODE.EXAMPLE.','bücher.example'];
const cases=[];
for(const host of hosts)for(const scheme of ['http','https'])for(const suffix of ['','/rpc/','?','#','/rpc?x=1']){
 const input=`${scheme}://${host}${suffix}`; cases.push({input,result:declaredEndpointURL(input)??null});
}
const output=JSON.stringify(cases,null,2)+'\n';
writeFileSync('tests/endpoints-typescript.json',output);
const hash=x=>createHash('sha256').update(x).digest('hex');
const paths=['packages/client/src/endpoints.ts','packages/client/src/node-client.ts','package-lock.json','packages/client/dist/endpoints.js'];
writeFileSync('tests/endpoints-typescript-evidence.json',JSON.stringify({repository:'0xLattice/lattice-sdk',revision,node:process.version,cases:cases.length,sha256:hash(output),sources:Object.fromEntries(paths.map(p=>[p,hash(readFileSync(resolve(root,p)))]))},null,2)+'\n');
