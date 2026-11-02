import {execFileSync} from 'node:child_process';
import {writeFileSync,readFileSync} from 'node:fs';
import {resolve,dirname} from 'node:path';
import {fileURLToPath,pathToFileURL} from 'node:url';
import {createHash} from 'node:crypto';
const repo=resolve(dirname(fileURLToPath(import.meta.url)),'..'),sdk=resolve(repo,'../lattice-sdk');
const sha=execFileSync('git',['-C',sdk,'rev-parse','HEAD'],{encoding:'utf8'}).trim();if(sha!=='3a1be07318f12b85f147e0a5ea55ae68a224e780')throw Error('wrong pin');
const {NodeClient,EndpointResolver}=await import(pathToFileURL(resolve(sdk,'packages/client/dist/index.js')));
const short=s=>s.length>64?'<long-name>':s;
const all=[];
const keepAlive=setInterval(()=>{},1000);
for(const mode of ['bfs_failover','oversize','root404','root503','invalid_names','stop_early','timeout_failover','root_timeout','halving_reset']){
 let requests=0;const directories=[];
 const fetch=async(input)=>{requests++;const url=new URL(input),base=url.origin,path=url.searchParams.get('chainPath').split('/'),joined=path.join('/'),route=url.pathname.split('/').at(-1);const send=(x,status=200,headers={})=>new Response(JSON.stringify(x),{status,headers});
  if(route==='genesis')return send({genesisHash:'network'});
  if(route==='info')return send({chain:path,height:'1',tipCID:'tip'});
  if(route==='endpoints'){const urls=path.length===2&&path[1]==='A'?['https://a1.example','https://a2.example']:[`https://${path.at(-1).length>64?'long':path.at(-1).toLowerCase()}.example`];return send({chainPath:path,endpoints:urls,submitEndpoints:urls.slice(0,1)});}
  if(route!=='children')throw Error('unexpected route');
  const limit=Number(url.searchParams.get('limit')),after=url.searchParams.get('after');directories.push({source:base,path:path.map(short),after:after===null?null:short(after),limit});
  if(path.length===1){
   if(mode==='halving_reset'){if(limit>100)return send({},200,{'content-length':'100000'});return send({chainPath:path,directories:after===null?['A']:['B'],...(after===null?{next:'A'}:{})});}
   if(mode==='root_timeout')return new Promise(()=>{});
   if(mode==='root404'||mode==='root503')return send({error:'failure'},mode==='root404'?404:503);
   if(mode==='oversize'){if(limit>1)return send({},200,{'content-length':'100000'});return send({chainPath:path,directories:['"'.repeat(40000)]});}
   if(mode==='invalid_names')return send({chainPath:path,directories:['','A/B','Valid']});
   return send({chainPath:path,directories:after===null?['A','B']:['C'],...(after===null?{next:'B'}:{})});
  }
  if(['bfs_failover','timeout_failover'].includes(mode)&&joined==='Nexus/A'){
   if(base==='https://a1.example')return after===null?send({chainPath:path,directories:['AA'],next:'AA'}):(mode==='timeout_failover'?new Promise(()=>{}):send({error:'down'},503));
   return send({chainPath:path,directories:['AB']});
  }
  if(['bfs_failover','timeout_failover'].includes(mode)&&joined==='Nexus/B')return send({chainPath:path,directories:['BA']});
  return send({chainPath:path,directories:[]});
 };
 const resolver=new EndpointResolver(new NodeClient('https://root.example',['Nexus']),{fetch,maximumRequests:1,timeoutMilliseconds:10});
 const walk=resolver.walk();const yielded=[];let error=null;
 try{for await(const e of walk){yielded.push({url:e.url,path:e.chainPath.map(short),submit:e.declaresSubmit});if(mode==='stop_early')break;}}
 catch(e){error=e.status??e.name;}
 all.push({mode,yielded,directories,requests,error});
}
clearInterval(keepAlive);
const file=resolve(repo,'tests/walk-typescript.json');writeFileSync(file,JSON.stringify(all,null,2)+'\n');
const hash=p=>createHash('sha256').update(readFileSync(p)).digest('hex');writeFileSync(resolve(repo,'tests/walk-typescript-evidence.json'),JSON.stringify({sdk_sha:sha,node:process.version,source_sha256:hash(resolve(sdk,'packages/client/src/endpoints.ts')),fixture_sha256:hash(file),scope:'Actual SDK EndpointResolver.walk with deterministic injected HTTP responses, including lazy early stop, BFS pages/failover, response-size halving and root errors.'},null,2)+'\n');console.log(`${all.length} actual TypeScript walk scenarios`);
