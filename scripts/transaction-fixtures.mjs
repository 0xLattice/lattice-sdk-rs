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
const cases=[];
for(const seed of [0,7,9])for(const [amount,fee] of [[1n,0n],[123456789n,4294967296n],[9223372036854775806n,1n]])for(const chainPath of [['Nexus'],['Nexus','Alpha'],['Nexus','é/😀']]){
 const account=core.accountFromPrivateKey(new Uint8Array(32).fill(seed));
 const recipient=core.accountFromPrivateKey(new Uint8Array(32).fill(seed+1));
 const nonce=18446744073709551615n;
 const body=core.buildTransfer({from:account.address,to:recipient.address,amount,fee,nonce,chainPath});
 const encoded=core.encodeTransactionBody(body),signed=core.signTransactionBody(body,account.privateKey);
 const signatures={[signed.publicKey]:signed.signature};
 const envelope=core.encodeSignedTransaction(signatures,encoded.cid);
 cases.push({seed,amount:String(amount),fee:String(fee),nonce:String(nonce),chainPath,address:account.address,recipient:recipient.address,publicKey:account.publicKey,bodyCID:encoded.cid,bodyHex:Buffer.from(encoded.bytes).toString('hex'),signature:signed.signature,transactionCID:envelope.cid,transactionHex:Buffer.from(envelope.bytes).toString('hex'),size:core.signedTransactionSize(body),growth:core.stateGrowthCeiling(body),payload:core.transactionPayload(signatures,body)});
}
const output=JSON.stringify(cases,null,2)+'\n';writeFileSync('tests/transactions-typescript.json',output);
const hash=x=>createHash('sha256').update(x).digest('hex');
const paths=['packages/core/src/transaction.ts','packages/core/src/signing.ts','packages/core/src/dag-cbor.ts','package-lock.json','packages/core/dist/transaction.js'];
writeFileSync('tests/transactions-typescript-evidence.json',JSON.stringify({repository:'0xLattice/lattice-sdk',revision,node:process.version,cases:cases.length,sha256:hash(output),sources:Object.fromEntries(paths.map(p=>[p,hash(readFileSync(resolve(root,p)))]))},null,2)+'\n');
