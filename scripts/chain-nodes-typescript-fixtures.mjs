import {execFileSync} from 'node:child_process';
import {readFileSync,writeFileSync} from 'node:fs';
import {resolve,dirname} from 'node:path';
import {fileURLToPath,pathToFileURL} from 'node:url';
import {createHash} from 'node:crypto';
const repo=resolve(dirname(fileURLToPath(import.meta.url)),'..'),sdk=resolve(repo,'../lattice-sdk');
const pin='3a1be07318f12b85f147e0a5ea55ae68a224e780';
if(execFileSync('git',['-C',sdk,'rev-parse','HEAD'],{encoding:'utf8'}).trim()!==pin)throw Error('wrong pin');
const {ChainNodes}=await import(pathToFileURL(resolve(sdk,'packages/client/dist/chain-nodes.js')));
const {NodeClient}=await import(pathToFileURL(resolve(sdk,'packages/client/dist/node-client.js')));
const {EndpointResolver}=await import(pathToFileURL(resolve(sdk,'packages/client/dist/endpoints.js')));
const {NodeError}=await import(pathToFileURL(resolve(sdk,'packages/client/dist/http.js')));
const A='https://a.example',B='https://b.example',C='https://c.example',T='Nexus/A';
const cases=[
 {name:'own_and_found_dedup',serves:{[A+'/'+T]:true,[B+'/'+T]:true,[A+'/Nexus']:true},declared:{[A]:[A,C]},reads:4},
 {name:'all_offline',serves:{},declared:{}},
 {name:'all_fail_first_error',serves:{[A+'/'+T]:503,[B+'/'+T]:502},declared:{}},
 {name:'partial_failure_empty',serves:{[A+'/'+T]:503},declared:{}},
 {name:'own_directory_failure',serves:{[A+'/'+T]:true,[A+'/Nexus']:true},declared:{[A]:502}},
 {name:'failed_walk_then_empty',serves:{[A+'/Nexus']:true,[B+'/Nexus']:true},declared:{[A]:503,[B]:[]}},
 {name:'empty_first_stops',serves:{[A+'/Nexus']:true,[B+'/Nexus']:true},declared:{[A]:[],[B]:[C]}},
 {name:'failed_walk_then_hosts',serves:{[A+'/Nexus']:true,[B+'/Nexus']:true},declared:{[A]:503,[B]:[C]}},
 {name:'nearest_ancestor',path:['Nexus','A','B'],serves:{[A+'/Nexus']:true},declared:{[A]:[C]}},
 {name:'random_round_robin',random:0.0000000004656612873077393,serves:{[A+'/'+T]:true,[B+'/'+T]:true},declared:{},reads:4},
 {name:'read_failover_last_error',serves:{[A+'/'+T]:true,[B+'/'+T]:true},declared:{},reads:3,readErrors:{[A]:501,[B]:502}},
 {name:'walk_rotates_on_expiry',serves:{[A+'/Nexus']:true,[B+'/Nexus']:true},declared:{[A]:[A],[B]:[B]},reads:3,advance:100},
 {name:'failed_discovery_retries',serves:{[A+'/'+T]:503,[B+'/'+T]:502},declared:{},reads:3},
];
const oldInfo=NodeClient.prototype.chainInfo,oldResolve=EndpointResolver.prototype.resolve;
for(const c of cases){
 const calls=[],readCalls=[],results=[];let now=0;
 NodeClient.prototype.chainInfo=async function(){const path=[...this.chainPath];calls.push({kind:'serves',url:this.baseURL,path});const result=c.serves[this.baseURL+'/'+path.join('/')];if(typeof result==='number')throw new NodeError(result,'fixture');if(result!==true)throw new NodeError(404,'absent');return {chain:path,height:1n,tipCID:'tip'};};
 // Resolver state is private: constructor NodeClient supplies public coordinates.
 let latest;
 // Store coordinates when ancestor serves successfully; resolve immediately follows.
 const info=NodeClient.prototype.chainInfo;
 NodeClient.prototype.chainInfo=async function(){const result=await info.call(this);latest={url:this.baseURL,path:[...this.chainPath]};return result;};
 EndpointResolver.prototype.resolve=async function(target){calls.push({kind:'declared',url:latest.url,ancestor:latest.path,path:[...target]});const value=c.declared[latest.url]??[];if(typeof value==='number')throw new NodeError(value,'directory');return value.map(url=>({url}));};
 const nodes=new ChainNodes([A+'/',B,A],{now:()=>now,random:()=>c.random??0,ttlMilliseconds:50});
 const path=c.path??['Nexus','A'];
 for(let i=0;i<(c.reads??1);i++){
  try{const list=await nodes.list(path);const chosen=await nodes.read(path,async n=>{readCalls.push(n.url);if(c.readErrors?.[n.url])throw new NodeError(c.readErrors[n.url],'read');return n;});results.push({list,chosen});}
  catch(e){results.push({error:e.status??e.name});}
  now+=c.advance??0;
 }
 c.expected={calls,readCalls,results};
}
NodeClient.prototype.chainInfo=oldInfo;EndpointResolver.prototype.resolve=oldResolve;
const output=resolve(repo,'tests/chain-nodes-typescript.json');writeFileSync(output,JSON.stringify(cases,null,2)+'\n');
const hash=p=>createHash('sha256').update(readFileSync(p)).digest('hex');writeFileSync(resolve(repo,'tests/chain-nodes-typescript-evidence.json'),JSON.stringify({sdk_sha:pin,node:process.version,source_sha256:hash(resolve(sdk,'packages/client/src/chain-nodes.ts')),fixture_sha256:hash(output),scope:'Actual pinned ChainNodes class. NodeClient.chainInfo and EndpointResolver.resolve replaced with deterministic injected seams; no network. Directory resolver independently covered by its own corpus.'},null,2)+'\n');
console.log(cases.length+' actual TypeScript ChainNodes scenarios');
