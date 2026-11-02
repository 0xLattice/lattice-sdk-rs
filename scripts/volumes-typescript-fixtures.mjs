// Actual pinned TypeScript SDK observations; no implementation extraction.
import {execFileSync} from 'node:child_process';
import {readFileSync,writeFileSync} from 'node:fs';
import {resolve,dirname} from 'node:path';
import {pathToFileURL,fileURLToPath} from 'node:url';
import {createHash} from 'node:crypto';
const repo=resolve(dirname(fileURLToPath(import.meta.url)),'..');
const sdk=resolve(process.argv[2]??resolve(repo,'../lattice-sdk'));
const sha='3a1be07318f12b85f147e0a5ea55ae68a224e780';
if(execFileSync('git',['-C',sdk,'rev-parse','HEAD'],{encoding:'utf8'}).trim()!==sha)throw Error('wrong SDK pin');
if(execFileSync('git',['-C',sdk,'status','--porcelain','--untracked-files=no'],{encoding:'utf8'}).trim())throw Error('tracked source modifications');
const v=await import(pathToFileURL(resolve(sdk,'packages/volumes/dist/index.js')));
const core=await import(pathToFileURL(resolve(sdk,'packages/core/dist/index.js')));
const rows=[];
const result=f=>{try{return {ok:true,...f()};}catch(e){return {ok:false,code:e.code??(e instanceof v.VolumeTransportError?'archive':'cbor')};}};
const entry=(bytes,codec=0x71)=>({cid:core.cidV1(codec,Uint8Array.from(bytes)),bytes:Array.from(bytes)});
const jsvolume=x=>({rootCID:x.root_cid,entries:x.entries.map(e=>({...e,bytes:Uint8Array.from(e.bytes)}))});
function verify(name,volume,expected=volume.root_cid,limits={maximumEntries:4096,maximumBytes:67108864}){rows.push({name,op:'verify',volume,expected,limits,result:result(()=>({size:v.verifyVolume(jsvolume(volume),expected,limits).size}))});}
const a=entry([0xa0]),b=entry([1],0x55);const volume={root_cid:a.cid,entries:[b,a]};
verify('sha256_any_codec',volume);verify('wrong_root',volume,b.cid);verify('missing_root',{root_cid:a.cid,entries:[b]});verify('duplicate',{root_cid:a.cid,entries:[a,a]});verify('count_limit',volume,a.cid,{maximumEntries:1,maximumBytes:99});verify('payload_limit',volume,a.cid,{maximumEntries:9,maximumBytes:1});verify('mismatch',{root_cid:a.cid,entries:[{cid:a.cid,bytes:[0]}]});
const varint=n=>{const out=[];while(n>=128){out.push((n%128)|128);n=Math.floor(n/128);}out.push(n);return out;};
const identity=bytes=>'b'+core.base32Encode(Uint8Array.from([1,0x55,0,...varint(bytes.length),...bytes]));
for(const n of [1,80]){const bytes=Array.from({length:n},(_,i)=>i);const e={cid:identity(bytes),bytes};verify('identity_'+n,{root_cid:e.cid,entries:[e]});}
for(const n of [5100,5120]){const bytes=Array(n).fill(17);const e={cid:identity(bytes),bytes};const volume={root_cid:e.cid,entries:[e]};verify('identity_'+n,volume);rows.push({name:'encode_identity_'+n,op:'encode',volume,result:result(()=>({archive:Array.from(v.encodeVolumeArchive(jsvolume(volume)))}))});}
const cidbytes=core.parseCID(a.cid).bytes;
for(const [name,cid] of [['uppercase',a.cid.toUpperCase()],['extra_zero_base32',a.cid+'a'],['truncated_sha','b'+core.base32Encode(Uint8Array.from([1,0x71,0x12,1,cidbytes.at(-1)]))],['empty_identity','b'+core.base32Encode(Uint8Array.of(1,0x71,0,0))],['noncanonical_varint','b'+core.base32Encode(Uint8Array.from([0x81,0,...cidbytes.slice(1)]))]])verify(name,{root_cid:cid,entries:[{cid,bytes:a.bytes}]});
const archive=Array.from(v.encodeVolumeArchive(jsvolume(volume)));
rows.push({name:'encode_sorted',op:'encode',volume,result:{ok:true,archive}});
function decode(name,bytes,root=a.cid,limits={maximumEntries:4096,maximumBytes:67108864}){rows.push({name,op:'decode',root,archive:bytes,limits,result:result(()=>({size:v.decodeVolumeArchive(root,Uint8Array.from(bytes),limits).entries.length}))});}
decode('decode_roundtrip',archive);decode('decode_missing_root',archive,'missing');decode('trailing',[...archive,0]);decode('zero_entries',[0,0]);decode('decode_count_limit',archive,a.cid,{maximumEntries:1,maximumBytes:99});decode('decode_payload_limit',archive,a.cid,{maximumEntries:9,maximumBytes:1});
for(let n=0;n<archive.length;n++)decode('truncated_'+n,archive.slice(0,n));
const one=Array.from(v.encodeVolumeArchive(jsvolume({root_cid:a.cid,entries:[a]})));
const duplicate=[0,2,...one.slice(2),...one.slice(2)];decode('duplicate_archive',duplicate);
const length=(one[2]<<8)|one[3];decode('bom_cid',[0,1,(length+3)>>8,(length+3)&255,0xef,0xbb,0xbf,...one.slice(4)]);
decode('invalid_utf8',[0,1,0,1,0xff,0,0,0,1,0]);decode('long_cid',[0,1,0x20,1]);
for(const [name,bytes] of [['null',[0xf6]],['uint64',[0x1b,255,255,255,255,255,255,255,255]],['negative',[0x3b,255,255,255,255,255,255,255,255]],['f64',[0xfb,0,0,0,0,0,0,0,0]],['nonminimal',[0x18,1]],['duplicate_map',[0xa2,0x61,0x61,1,0x61,0x61,2]],['unordered_map',[0xa2,0x61,0x62,1,0x61,0x61,2]],['tag',[0xc0,1]],['depth80',[...Array(80).fill(0x81),0]],['text_bom',[0x63,0xef,0xbb,0xbf]]])rows.push({name:'cbor_'+name,op:'cbor',bytes,result:result(()=>{core.decodeDagCbor(Uint8Array.from(bytes));return {};})});
function proof(name,node,key,value,extras=[]){const root=entry(core.encodeDagCbor(node));const p={rootCID:root.cid,key,...(value===undefined?{}:{value:BigInt(value)}),entries:[root,...extras].map(e=>({...e,bytes:Uint8Array.from(e.bytes)}))};rows.push({name,op:'proof',root:root.cid,key,value:value??null,entries:[root,...extras],valid:v.verifyUint64SparseProof(p)});}
proof('proof_present',{prefix:'alice',children:[],value:9n},'alice','9');proof('proof_wrong_value',{prefix:'alice',children:[],value:9n},'alice','8');proof('proof_absent',{prefix:'alice',children:[],value:9n},'bob',undefined);
const leaf=entry(core.encodeDagCbor({prefix:'a',children:[],value:9n}));const branch={children:[{key:'a',value:{rawCID:leaf.cid}}]};proof('proof_branch',branch,'a','9',[leaf]);proof('proof_missing_branch',branch,'a','9');proof('proof_duplicate_matching',{children:[branch.children[0],branch.children[0]]},'a','9',[leaf]);proof('proof_astral_edge',{children:[{key:'😀',value:{rawCID:leaf.cid}}]},'😀',undefined,[leaf]);
const output=resolve(repo,'tests/volumes-typescript.json');writeFileSync(output,JSON.stringify(rows,null,2)+'\n');
const hash=path=>createHash('sha256').update(readFileSync(path)).digest('hex');
writeFileSync(resolve(repo,'tests/volumes-typescript-evidence.json'),JSON.stringify({sdk_sha:sha,node:process.version,cases:rows.length,source_sha256:Object.fromEntries(['packages/volumes/src/index.ts','packages/core/src/cid.ts','packages/core/src/dag-cbor.ts','package-lock.json'].map(p=>[p,hash(resolve(sdk,p))])),fixture_sha256:hash(output),scope:'Actual TypeScript verifyVolume/archive/CBOR/sparse proof outputs. HTTP transport separately tested via loopback. Rust explicit configurable CBOR nesting cap is additional resource policy.'},null,2)+'\n');
console.log(`${rows.length} actual TypeScript cases written`);
