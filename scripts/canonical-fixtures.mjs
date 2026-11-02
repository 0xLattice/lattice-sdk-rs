import {execFileSync} from 'node:child_process';
import {readFileSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
import {createHash} from 'node:crypto';
const root=resolve(process.argv[2]??'../lattice-sdk');
const revision='3a1be07318f12b85f147e0a5ea55ae68a224e780';
if(execFileSync('git',['rev-parse','HEAD'],{cwd:root,encoding:'utf8'}).trim()!==revision)throw Error('source pin mismatch');
if(execFileSync('git',['status','--porcelain','--untracked-files=no'],{cwd:root,encoding:'utf8'}).trim())throw Error('source modified');
execFileSync('npm',['run','build'],{cwd:root,stdio:'inherit'});
const core=await import(pathToFileURL(resolve(root,'packages/core/dist/index.js')));
const link=rawCID=>({rawCID});
const base={version:1n,transactions:link('tx'),target:'0x'+'f'.repeat(64),nextTarget:'0x0001',spec:link('spec'),parentState:link('parentstate'),prevState:link('prev'),postState:link('post'),children:link('children'),height:0n,timestamp:0n,nonce:0n};
const cases=[];
function add(kind,name,value){const bytes=core.encodeDagCbor(value);let result=null,pow=null;
 try{result=({block:core.decodeCanonicalBlock,state:core.decodeCanonicalLatticeState,transaction:core.decodeCanonicalSignedTransaction})[kind](bytes);
 if(kind==='block'){try{pow={preimage:Buffer.from(core.canonicalBlockProofOfWorkPreimage(result)).toString('hex'),hash:Buffer.from(core.canonicalBlockProofOfWorkHash(result)).toString('hex'),valid:core.verifyNexusBlockProofOfWork(result)};}catch{}}
 }catch{}
 cases.push({kind,name,bytes:Buffer.from(bytes).toString('hex'),result,pow});}
add('block','baseline',base);
for(const [key,values]of Object.entries({version:[-1n,65535n,65536n,'1',null],height:[-1n,0xffffffffffffffffn,'1'],timestamp:[-0x8000000000000001n,-0x8000000000000000n,0x7fffffffffffffffn,0x8000000000000000n],nonce:[-1n,0xffffffffffffffffn],parent:[null,link('parent'),{rawCID:42}],rewardRecipient:[null,'','雪'],target:['0x0','0x01','0X1','0xAB','1'],nextTarget:['0x0','0xzz']}))for(const value of values)add('block',`${key}:${String(value)}`,{...base,[key]:value});
for(const key of ['version','children','height','nonce']){const b={...base};delete b[key];add('block',`missing:${key}`,b);}
add('block','extra fields',{...base,unknown:'accepted',transactions:{rawCID:'tx',extra:true}});
const state={accountState:link('account'),generalState:link('general'),depositState:link('deposit'),receiptState:link('receipt')};
add('state','valid',state);for(const key of Object.keys(state)){add('state',`null:${key}`,{...state,[key]:null});const s={...state};delete s[key];add('state',`missing:${key}`,s);}
for(const [name,signatures]of [['empty',[]],['ordered',[{key:'a',value:'x'},{key:'b',value:'y'}]],['duplicate',[{key:'a',value:'x'},{key:'a',value:'y'}]],['reverse',[{key:'b',value:'x'},{key:'a',value:'y'}]],['utf16',[{key:'😀',value:'x'},{key:'\ue000',value:'y'}]],['utf8-order',[{key:'\ue000',value:'x'},{key:'😀',value:'y'}]],['null',null],['numeric',[{key:'a',value:1n}]]])add('transaction',name,{body:link('body'),signatures});
const targets=['0x0','0x1','0x0001','0x2','0x'+ 'f'.repeat(64),'0x'+ 'f'.repeat(65),'0xAB','0X1','1','0x','0x10000000000000000'];
const work=targets.map(target=>{let result=null;try{result=core.workForTarget(target).toString();}catch{}return{target,result};});
const output=JSON.stringify({cases,work},(_,v)=>typeof v==='bigint'?v.toString():v,2)+'\n';writeFileSync('tests/canonical-typescript.json',output);
const hash=x=>createHash('sha256').update(x).digest('hex');
const paths=['packages/core/src/canonical-values.ts','packages/core/src/dag-cbor.ts','packages/core/dist/canonical-values.js','package-lock.json'];
writeFileSync('tests/canonical-typescript-evidence.json',JSON.stringify({repository:'0xLattice/lattice-sdk',revision,node:process.version,cases:cases.length,work:work.length,sha256:hash(output),sources:Object.fromEntries(paths.map(p=>[p,hash(readFileSync(resolve(root,p)))]))},null,2)+'\n');
