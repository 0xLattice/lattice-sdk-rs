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
const {transactionWeight,feeToOutbid}=await import(pathToFileURL(resolve(root,'packages/client/dist/models.js')));
const weights=[null,...[
[1000,100,10,20],[1000,0,0,0],[0,100,10,20],[-1,-1,-1,-1],[1000,100,10,-20],
[9007199254740991,9007199254740991,5000,100],[1000000,1000000,5000,200]
].map(([blockRoom,growthRoom,maxTransactions,transactionAllowance])=>({blockRoom,growthRoom,maxTransactions,transactionAllowance}))];
const cases=[];
for(const weight of weights)for(const size of [0,1,300,1001])for(const growth of [-1,0,11,100,1001,9007199254740991]){
 cases.push({size,growth,weight,result:String(transactionWeight(size,growth,weight??undefined))});
}
const rates=[];
for(const lowestFee of ['0','1','10','18446744073709551615'])for(const lowestFeeWeight of [1,300,1000])for(const weight of [1,300,450,9007199254740991]){
 rates.push({lowestFee,lowestFeeWeight,weight,result:feeToOutbid({lowestFee:BigInt(lowestFee),lowestFeeWeight},weight).toString()});
}
const output=JSON.stringify({cases,rates},null,2)+'\n';
writeFileSync('tests/fee-typescript.json',output);
const hash=x=>createHash('sha256').update(x).digest('hex');
const paths=['packages/client/src/models.ts','packages/client/src/wire.ts','package-lock.json','packages/client/dist/models.js','packages/client/dist/wire.js'];
writeFileSync('tests/fee-typescript-evidence.json',JSON.stringify({repository:'0xLattice/lattice-sdk',revision,node:process.version,cases:cases.length,rates:rates.length,sha256:hash(output),sources:Object.fromEntries(paths.map(p=>[p,hash(readFileSync(resolve(root,p)))]))},null,2)+'\n');
