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
const core=await import(pathToFileURL(resolve(root,'packages/core/dist/index.js')));
const {SponsorClient}=await import(pathToFileURL(resolve(root,'packages/relay/dist/index.js')));
const client=core.accountFromPrivateKey(new Uint8Array(32).fill(7));
const sponsor=core.accountFromPrivateKey(new Uint8Array(32).fill(9));
const draft=core.buildTransfer({from:client.address,to:'recipient',amount:10n,fee:0n,nonce:3n,chainPath:['Nexus']});
const body={...draft,accountActions:[...draft.accountActions,{owner:client.address,delta:2n},{owner:sponsor.address,delta:-7n}],signers:[client.address,sponsor.address]};
const signed=core.signTransactionBody(body,sponsor.privateKey);
const baseline={fee:'5',pay:'2',transaction:core.transactionPayload({[sponsor.publicKey]:signed.signature},body).transaction};
const mutations=[
 ['valid',()=>{}],['changed-pay',a=>a.pay='3'],['noncanonical-fee',a=>a.fee='05'],['negative-fee',a=>a.fee='-1'],
 ['wrong-nonce',a=>a.transaction.body.nonce='4'],['wrong-path',a=>a.transaction.body.chainPath=['Nexus','Child']],
 ['extra-signer',a=>a.transaction.body.signers.push('extra')],['same-sponsor',a=>a.transaction.body.signers[1]=client.address],
 ['extra-action',a=>a.transaction.body.accountActions.push({owner:'thief',delta:'1'})],
 ['changed-credit',a=>a.transaction.body.accountActions[2].delta='3'],['extra-field',a=>a.transaction.body.unknown=true],
 ['missing-signature',a=>a.transaction.signatures={}],['wrong-key',a=>a.transaction.signatures={[client.publicKey]:signed.signature}],
 ['two-signatures',a=>a.transaction.signatures[client.publicKey]=signed.signature],['nonstring-signature',a=>a.transaction.signatures[sponsor.publicKey]=4],
 ['opaque-signature-retained',a=>a.transaction.signatures[sponsor.publicKey]='not-yet-verified'],
 ['overflow-fee',a=>a.fee='9223372036854775808'],['null-transaction',a=>a.transaction=null]
];
const cases=[];
for(const [name,mutate]of mutations){const answer=structuredClone(baseline);mutate(answer);let accepted=false;
 const service=new SponsorClient('https://sponsor.example',{fetch:async()=>new Response(JSON.stringify(answer))});
 try{await service.sponsorTransaction('test-client',draft,2n);accepted=true;}catch{}
 cases.push({name,answer,accepted});
}
const output=JSON.stringify({draft:core.toWireTransactionBody(draft),pay:'2',cases},null,2)+'\n';
writeFileSync('tests/sponsor-typescript.json',output);
const hash=x=>createHash('sha256').update(x).digest('hex');
const paths=['packages/relay/src/sponsor.ts','packages/core/src/transaction.ts','packages/core/src/address.ts','package-lock.json','packages/relay/dist/sponsor.js'];
writeFileSync('tests/sponsor-typescript-evidence.json',JSON.stringify({repository:'0xLattice/lattice-sdk',revision,node:process.version,cases:cases.length,sha256:hash(output),sources:Object.fromEntries(paths.map(p=>[p,hash(readFileSync(resolve(root,p)))]))},null,2)+'\n');

const {shareHash,grind}=await import(pathToFileURL(resolve(root,'packages/relay/dist/index.js')));
const hashes=[];
for(const prefix of ['', '0x', '00', '0xdeadbeef', 'aa'.repeat(63), 'ab'.repeat(64), 'cd'.repeat(65), 'AB', '0Xaa', 'a', 'zz']){
 for(const nonce of [0n,1n,-1n,(1n<<64n)-1n,1n<<64n,(1n<<64n)+1n,-(1n<<65n)-7n]){
  let hash=null;try{hash=Buffer.from(shareHash(prefix,nonce)).toString('hex');}catch{}
  hashes.push({prefix,nonce:nonce.toString(),hash});
 }
}
const searches=[];
for(const [prefix,start,count,target]of [
 ['',0n,0n,0n],['00',0n,16n,(1n<<256n)-1n],['deadbeef',(1n<<64n)-3n,8n,(1n<<256n)-1n],
 ['abcdef',17n,100n,(1n<<252n)-1n],['',1n<<100n,30n,1n<<255n],['aa',0n,10n,0n]
]){
 const found=[];
 for await(const nonce of grind({prefixHex:prefix,startNonce:start,count,shareTarget:target,expiresAt:Infinity})) found.push(nonce.toString());
 searches.push({prefix,start:start.toString(),count:count.toString(),target:target.toString(),found});
}
const sharesOutput=JSON.stringify({hashes,searches},null,2)+'\n';
writeFileSync('tests/sponsor-shares-typescript.json',sharesOutput);
writeFileSync('tests/sponsor-shares-typescript-evidence.json',JSON.stringify({repository:'0xLattice/lattice-sdk',revision,node:process.version,hashes:hashes.length,searches:searches.length,sha256:hash(sharesOutput),sources:Object.fromEntries(paths.map(p=>[p,hash(readFileSync(resolve(root,p)))]))},null,2)+'\n');

const workBaseline={client:'client',assignment:'assignment',prefixHex:'0xdeadbeef',shareTarget:'0f',startNonce:'18446744073709551616',count:'20',fee:'5',pay:'2',sharesForFee:'7',expiresInMilliseconds:'30000'};
const workMutations=[['valid',()=>{}],['empty-prefix',a=>a.prefixHex=''],['upper-prefix',a=>a.prefixHex='AB'],['odd-prefix',a=>a.prefixHex='a'],['upper-target',a=>a.shareTarget='AB'],['empty-target',a=>a.shareTarget=''],['large-target',a=>a.shareTarget='f'.repeat(65)],['zero-target',a=>a.shareTarget='0'],['leading-target',a=>a.shareTarget='000f'],['numeric-count',a=>a.count=2],['negative-count',a=>a.count='-1'],['leading-count',a=>a.count='02'],['huge-count',a=>a.count='1'+'0'.repeat(100)],['null-client',a=>a.client=null],['missing-assignment',a=>delete a.assignment],['zero-expiry',a=>a.expiresInMilliseconds='0'],['noncanonical-expiry',a=>a.expiresInMilliseconds='00']];
const workCases=[];
for(const [name,mutate] of workMutations){const answer=structuredClone(workBaseline);mutate(answer);let accepted=false;try{await new SponsorClient('https://sponsor.example',{fetch:async()=>new Response(JSON.stringify(answer))}).requestWork();accepted=true;}catch{}workCases.push({name,answer,accepted});}
const statusBaseline={root:'root',bytes:'184467440737095516160',needed:{Nexus:'2','Nexus/Child':'5'},credit:'1',stored:false};
const statusMutations=[['valid',()=>{}],['array-needed',a=>a.needed=['2','3']],['empty-array',a=>a.needed=[]],['null-needed',a=>a.needed=null],['numeric-needed',a=>a.needed.Nexus=2],['negative-needed',a=>a.needed.Nexus='-1'],['leading-bytes',a=>a.bytes='01'],['boolean-string',a=>a.stored='true'],['stored',a=>a.stored=true],['null-root',a=>a.root=null]];
const statusCases=[];
for(const [name,mutate]of statusMutations){const answer=structuredClone(statusBaseline);mutate(answer);let accepted=false;try{await new SponsorClient('https://sponsor.example',{fetch:async()=>new Response(JSON.stringify(answer))}).storeStatus('root');accepted=true;}catch{}statusCases.push({name,answer,accepted});}
const httpOutput=JSON.stringify({work:workCases,status:statusCases},null,2)+'\n';
writeFileSync('tests/sponsor-http-typescript.json',httpOutput);
writeFileSync('tests/sponsor-http-typescript-evidence.json',JSON.stringify({repository:'0xLattice/lattice-sdk',revision,node:process.version,cases:workCases.length+statusCases.length,sha256:hash(httpOutput),sources:Object.fromEntries(paths.map(p=>[p,hash(readFileSync(resolve(root,p)))]))},null,2)+'\n');
